//! Subject access (SPEC §12.3, GDPR Art. 15 / FADP Art. 25).
//!
//! `cyberbrain policy subject <identifier>` searches every note, block and audit row for an
//! identifier (a name, an email, a handle) and produces a report with citations that can be
//! handed to the data subject.
//!
//! The storage side is a trait ([`SubjectSource`]) the index implements
//! (`Index::blocks_containing` is the natural fit). This side re-checks every block with
//! its own case-folded search so an FTS over-match is trimmed, computes byte ranges for
//! highlighting, searches the audit log, and writes the `subject.access` row.
//!
//! **The audit row carries a hash of the identifier, not the identifier.** A subject-access
//! request must not itself plant another copy of the person's name in a durable log. A
//! later request for the same identifier still finds the earlier one, by hash.
//!
//! Matching is substring, case-folded. It over-returns on purpose (`ian` finds `Christian`)
//! and marks each occurrence as whole-word or not; under-returning on an Art. 15 request is
//! the worse failure, and the operator reviews the report before handing it over.

use crate::audit::{Actor, AuditAction, AuditFilter, AuditLog};
use crate::profile::{Profile, ProfileExt};
use cyberbrain_core::{Error, NoteId, Result, Ring};
use serde::Serialize;
use serde_json::json;

const MIN_IDENTIFIER_CHARS: usize = 3;
const EXCERPT_CONTEXT: usize = 60;

/// The thing being searched for, normalised once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identifier {
    raw: String,
    folded: String,
}

impl Identifier {
    pub fn new(raw: &str) -> Result<Self> {
        let raw = raw.trim().to_string();
        if raw.chars().count() < MIN_IDENTIFIER_CHARS {
            return Err(Error::Config(format!(
                "identifier {raw:?} is shorter than {MIN_IDENTIFIER_CHARS} characters; it would match everything"
            )));
        }
        let folded = raw.to_lowercase();
        Ok(Self { raw, folded })
    }

    pub fn raw(&self) -> &str {
        &self.raw
    }

    /// blake3 of the folded identifier, for the audit row.
    pub fn hash(&self) -> String {
        format!(
            "id:blake3:{}",
            &blake3::hash(self.folded.as_bytes()).to_hex()[..32]
        )
    }

    pub fn matches(&self, text: &str) -> bool {
        !self.occurrences(text).is_empty()
    }

    /// Every case-folded occurrence as byte ranges into `text`, with whether it stands as
    /// a whole word.
    pub fn occurrences(&self, text: &str) -> Vec<Occurrence> {
        // Fold with a byte map back to the original, since lowercasing can change byte
        // lengths (e.g. 'İ' -> "i̇").
        let mut lower = String::with_capacity(text.len());
        let mut map: Vec<usize> = Vec::with_capacity(text.len() + 1); // lower byte -> original byte
        for (i, c) in text.char_indices() {
            for lc in c.to_lowercase() {
                let start = lower.len();
                lower.push(lc);
                for _ in start..lower.len() {
                    map.push(i);
                }
            }
        }
        map.push(text.len());
        let mut out = Vec::new();
        for (ls, _) in lower.match_indices(self.folded.as_str()) {
            let le = ls + self.folded.len();
            let start = map[ls];
            let last_orig = map[le - 1];
            let end = last_orig + text[last_orig..].chars().next().map_or(0, char::len_utf8);
            let before = text[..start].chars().next_back();
            let after = text[end..].chars().next();
            let whole_word = !before.is_some_and(|c| c.is_alphanumeric())
                && !after.is_some_and(|c| c.is_alphanumeric());
            out.push(Occurrence {
                start,
                end,
                whole_word,
            });
        }
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Occurrence {
    pub start: usize,
    pub end: usize,
    pub whole_word: bool,
}

/// A block as the index hands it back. `note_id` and `ring` are optional because a
/// lexical-only lookup may not have them at hand; the citation is mandatory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubjectBlock {
    pub citation: String,
    pub note_id: Option<NoteId>,
    pub note_name: String,
    pub ring: Option<Ring>,
    pub text: String,
}

/// The storage seam. Return every block that *may* contain the identifier; this side
/// re-checks and trims. Over-returning is fine, under-returning is not.
pub trait SubjectSource {
    fn blocks_mentioning(&self, identifier: &Identifier) -> Result<Vec<SubjectBlock>>;
}

#[derive(Debug, Clone, Serialize)]
pub struct NoteHit {
    pub citation: String,
    pub note_name: String,
    pub ring: Option<Ring>,
    pub occurrences: Vec<Occurrence>,
    /// Text around the first occurrence, for the report.
    pub excerpt: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuditHit {
    pub ts: jiff::Timestamp,
    pub actor: String,
    pub action: String,
    pub subject: String,
    /// Whether the row matched by hash (an earlier subject-access request) rather than text.
    pub by_hash: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SubjectAccessReport {
    pub identifier: String,
    pub identifier_hash: String,
    pub generated_at: jiff::Timestamp,
    pub profile: Profile,
    pub law: &'static str,
    pub response_deadline: String,
    pub note_hits: Vec<NoteHit>,
    pub audit_hits: Vec<AuditHit>,
    /// Blocks the source returned that did not actually contain the identifier.
    pub trimmed: usize,
    pub caveats: Vec<String>,
}

/// Run the search and record it.
pub fn subject_access(
    audit: &AuditLog,
    actor: &Actor,
    profile: Profile,
    source: &dyn SubjectSource,
    ident: &Identifier,
) -> Result<SubjectAccessReport> {
    let mut note_hits = Vec::new();
    let mut trimmed = 0;
    for b in source.blocks_mentioning(ident)? {
        let occ = ident.occurrences(&b.text);
        if occ.is_empty() {
            trimmed += 1;
            continue;
        }
        note_hits.push(NoteHit {
            citation: b.citation,
            note_name: b.note_name,
            ring: b.ring,
            excerpt: excerpt(&b.text, occ[0]),
            occurrences: occ,
        });
    }
    note_hits.sort_by(|a, b| a.ring.cmp(&b.ring).then(a.note_name.cmp(&b.note_name)));

    let hash = ident.hash();
    let mut audit_hits = Vec::new();
    for row in audit.read(&AuditFilter {
        contains: Some(ident.raw().to_string()),
        ..Default::default()
    })? {
        audit_hits.push(AuditHit {
            ts: row.ts,
            actor: row.actor,
            action: row.action,
            subject: row.subject,
            by_hash: false,
        });
    }
    for row in audit.read(&AuditFilter {
        subject: Some(hash.clone()),
        ..Default::default()
    })? {
        audit_hits.push(AuditHit {
            ts: row.ts,
            actor: row.actor,
            action: row.action,
            subject: row.subject,
            by_hash: true,
        });
    }
    audit_hits.sort_by_key(|h| h.ts);

    let report = SubjectAccessReport {
        identifier: ident.raw().to_string(),
        identifier_hash: hash.clone(),
        generated_at: jiff::Timestamp::now(),
        profile,
        law: profile.law(),
        response_deadline: profile.access_response_deadline().to_string(),
        note_hits,
        audit_hits,
        trimmed,
        caveats: vec![
            "Matching is a case-insensitive substring search over note blocks and audit rows. It finds the identifier as written; it does not find a person described without it.".into(),
            "The files under notes/ are authoritative. If the index is stale, run `cyberbrain scan` and repeat.".into(),
            "Audit rows never store personal data from notes, so audit hits are rows that name the identifier in a subject field or an earlier request for it (by hash).".into(),
        ],
    };
    audit.record(
        actor,
        AuditAction::SubjectAccess,
        hash,
        json!({
            "profile": profile,
            "note_hits": report.note_hits.len(),
            "audit_hits": report.audit_hits.len(),
            "trimmed": trimmed,
            "response_deadline": report.response_deadline,
        }),
    )?;
    Ok(report)
}

fn excerpt(text: &str, occ: Occurrence) -> String {
    let mut start = occ.start.saturating_sub(EXCERPT_CONTEXT);
    while !text.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = (occ.end + EXCERPT_CONTEXT).min(text.len());
    while !text.is_char_boundary(end) {
        end += 1;
    }
    let mut s = String::new();
    if start > 0 {
        s.push('…');
    }
    s.push_str(text[start..end].replace('\n', " ").trim());
    if end < text.len() {
        s.push('…');
    }
    s
}

impl SubjectAccessReport {
    /// The form to hand over: Markdown, with citations.
    pub fn render_markdown(&self) -> String {
        let mut s = format!(
            "# Subject access report\n\nIdentifier: `{}`\nGenerated: {}\nProfile: {} ({})\nResponse deadline: {}\n\n",
            self.identifier,
            self.generated_at,
            self.profile.as_str(),
            self.law,
            self.response_deadline
        );
        s.push_str(&format!("## Notes ({} block(s))\n\n", self.note_hits.len()));
        if self.note_hits.is_empty() {
            s.push_str("No block contains the identifier.\n\n");
        }
        for h in &self.note_hits {
            let ring = h.ring.map(|r| r.to_string()).unwrap_or_else(|| "r?".into());
            let partial = h.occurrences.iter().filter(|o| !o.whole_word).count();
            s.push_str(&format!(
                "- **{}** ({ring}) `{}` — {} occurrence(s){}\n  > {}\n",
                h.note_name,
                h.citation,
                h.occurrences.len(),
                if partial > 0 {
                    format!(", {partial} inside a longer word")
                } else {
                    String::new()
                },
                h.excerpt
            ));
        }
        s.push_str(&format!(
            "\n## Audit log ({} row(s))\n\n",
            self.audit_hits.len()
        ));
        for a in &self.audit_hits {
            s.push_str(&format!(
                "- {} `{}` {} {}{}\n",
                a.ts,
                a.action,
                a.actor,
                a.subject,
                if a.by_hash {
                    " (earlier request for this identifier)"
                } else {
                    ""
                }
            ));
        }
        s.push_str("\n## Caveats\n\n");
        for c in &self.caveats {
            s.push_str(&format!("- {c}\n"));
        }
        if self.trimmed > 0 {
            s.push_str(&format!("- {} block(s) the index proposed did not contain the identifier and were dropped.\n", self.trimmed));
        }
        s
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("report serialises")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed(Vec<SubjectBlock>);
    impl SubjectSource for Fixed {
        fn blocks_mentioning(&self, _: &Identifier) -> Result<Vec<SubjectBlock>> {
            Ok(self.0.clone())
        }
    }

    fn block(citation: &str, name: &str, ring: Ring, text: &str) -> SubjectBlock {
        SubjectBlock {
            citation: citation.into(),
            note_id: None,
            note_name: name.into(),
            ring: Some(ring),
            text: text.into(),
        }
    }

    #[test]
    fn identifier_rejects_the_too_short() {
        assert!(Identifier::new("ab").is_err());
        assert!(Identifier::new("  ").is_err());
        assert!(Identifier::new("bob").is_ok());
    }

    #[test]
    fn occurrences_are_case_folded_byte_ranges_with_word_flags() {
        let id = Identifier::new("Müller").unwrap();
        let text = "Frau MÜLLER und Herr müller-Schmidt, nicht Müllerin.";
        let occ = id.occurrences(text);
        assert_eq!(occ.len(), 3);
        for o in &occ {
            assert_eq!(text[o.start..o.end].to_lowercase(), "müller");
        }
        assert_eq!(
            occ.iter().map(|o| o.whole_word).collect::<Vec<_>>(),
            [true, true, false]
        );
        assert!(id.matches("x müller y"));
        assert!(!id.matches("Meier"));
    }

    #[test]
    fn report_trims_over_matches_sorts_by_ring_and_records_a_hash() {
        let (log, sink) = AuditLog::in_memory();
        log.record(
            &Actor::Operator,
            AuditAction::NoteWrite,
            "note:01ARZ3NDEKTSV4RRFFQ69G5FAV",
            json!({"name": "bob-smith-onboarding", "title": "Onboarding Bob Smith"}),
        )
        .unwrap();
        let src = Fixed(vec![
            block(
                "r3-aaaaaaaaaaaa",
                "session-1",
                Ring::Session,
                "Talked to Bob Smith about the deployment.\nHe prefers email.",
            ),
            block(
                "r2-bbbbbbbbbbbb",
                "team",
                Ring::Knowledge,
                "bob.smith@corp.example.org owns the pager.",
            ),
            block(
                "r2-cccccccccccc",
                "unrelated",
                Ring::Knowledge,
                "The FTS index thought this matched. It does not.",
            ),
        ]);
        let id = Identifier::new("Bob Smith").unwrap();
        let r = subject_access(&log, &Actor::Operator, Profile::Eu, &src, &id).unwrap();
        assert_eq!(
            r.note_hits.len(),
            1,
            "email form is a different identifier; the FTS false positive is trimmed"
        );
        assert_eq!(r.trimmed, 2);
        assert_eq!(r.note_hits[0].citation, "r3-aaaaaaaaaaaa");
        assert!(r.note_hits[0].excerpt.contains("Bob Smith"));
        assert_eq!(
            r.audit_hits.len(),
            1,
            "the note.write detail named Bob Smith"
        );
        assert_eq!(
            r.response_deadline,
            "within one month (extendable by two months)"
        );

        let rows = sink.rows();
        let last = rows.last().unwrap();
        assert_eq!(last.action, "subject.access");
        assert!(last.subject.starts_with("id:blake3:"));
        assert!(
            !serde_json::to_string(last)
                .unwrap()
                .to_lowercase()
                .contains("bob smith"),
            "the identifier must not be in the log"
        );

        // A second request finds the first by hash.
        let r2 = subject_access(&log, &Actor::Operator, Profile::Ch, &src, &id).unwrap();
        assert!(
            r2.audit_hits
                .iter()
                .any(|h| h.by_hash && h.action == "subject.access")
        );
        assert_eq!(r2.response_deadline, "within 30 days");

        let md = r.render_markdown();
        assert!(md.contains("# Subject access report"));
        assert!(md.contains("`r3-aaaaaaaaaaaa`"));
        assert!(md.contains("## Caveats"));
        assert!(md.contains("2 block(s) the index proposed"));
    }

    #[test]
    fn excerpt_respects_char_boundaries() {
        let text = "ä".repeat(100) + "Bob" + &"ö".repeat(100);
        let id = Identifier::new("bob").unwrap();
        let occ = id.occurrences(&text)[0];
        let e = excerpt(&text, occ);
        assert!(e.contains("Bob"));
        assert!(e.starts_with('…') && e.ends_with('…'));
    }
}
