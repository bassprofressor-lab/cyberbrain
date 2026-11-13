//! A person's sign-off on a note's content (2026-11-13; claude-mem's "Checked" next to the
//! agent's "done").
//!
//! An agent writes most notes, and a reader cannot tell one an agent believed from one a
//! person checked. `cyberbrain signoff <note>` records that a person read the content and
//! stands behind it: a `note.signoff` row in the audit chain with who (this machine's
//! identity, as on a review), when, and the blake3 of the body as it was signed. Recall and
//! the page show "checked by X on …" while the body still hashes to that, and nothing once it
//! changed: a sign-off is on text, not on a name.
//!
//! Who may: the same boundary that keeps rings 0 and 1 the operator's. The actor must be the
//! operator at the CLI (`cli_actor`: the agent marker in the environment, then the process
//! tree), never an agent, MCP or a hook; a served store, which cannot tell who is asking,
//! refuses it unless started with `--allow-resident-writes` — and the CLI asks the person to
//! type the note's name at a terminal, which an agent's shell does not have. Nothing in MCP
//! offers it. What this does not stop, as for ring 0: a process of the same user that writes
//! the audit database itself. Only rows written by `operator` or `cli` are read back.
//!
//! Reading costs one indexed query on the audit log's `action` column per recall with hits,
//! and one note read per hit note that has a sign-off at all.

use super::*;
use cyberbrain_core::{Hit, SignOff};
use std::collections::HashMap;

/// The audit action. Like `resident.attest`, written raw: the policy crate's closed
/// vocabulary is for what the policy layer itself decides.
pub const NOTE_SIGNOFF: &str = "note.signoff";

/// What a sign-off is over: the body, byte for byte. The head changes for reasons that are
/// not content (`updated`, anchors renewed, links rewritten by `scan`); the text is what a
/// person read.
pub fn signed_hash(body: &str) -> String {
    blake3::hash(body.as_bytes()).to_hex().to_string()
}

#[derive(Debug, Clone, Serialize)]
pub struct SignOffReport {
    pub name: String,
    pub ring: Ring,
    pub by: String,
    pub at: jiff::Timestamp,
    /// blake3 of the body that was signed; absent for a withdrawal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blake3: Option<String>,
    pub withdrawn: bool,
    pub dry_run: bool,
    pub audit_preview: Vec<String>,
}

/// The last sign-off row for a note: who, when, over which body.
#[derive(Debug, Clone)]
struct Signed {
    by: String,
    at: jiff::Timestamp,
    hash: String,
}

impl App {
    /// Whether this caller may sign off at all. Asked before the CLI prompts, so an agent is
    /// told at once rather than after typing a name.
    pub fn may_sign_off(&self) -> Result<()> {
        let profile = format!("{:?}", self.policy.profile()).to_lowercase();
        if !matches!(self.actor, Actor::Operator | Actor::Cli) {
            return Err(Error::PolicyRefusal {
                profile,
                reason: format!(
                    "a sign-off says a person checked the content; {} is not one. Ask the \
                     operator to run `cyberbrain signoff <note>` in their own terminal",
                    self.actor
                ),
            });
        }
        if self.refuses_resident_writes() {
            return Err(Error::PolicyRefusal {
                profile,
                reason: "this served store cannot tell who is asking, so it records no \
                         sign-off; run `cyberbrain signoff <note>` in a terminal"
                    .into(),
            });
        }
        Ok(())
    }

    /// Record (or, with `withdraw`, take back) a person's sign-off on a note's content.
    /// `by` is this machine's identity (`identity::who`), asked by the caller as for a review.
    pub fn sign_off(
        &self,
        name: &str,
        by: &str,
        withdraw: bool,
        dry_run: bool,
    ) -> Result<SignOffReport> {
        self.may_sign_off()?;
        let name = frontmatter::normalize_name(name.trim()).into_owned();
        let note = self.store.read(&name)?;
        let by = by.trim().to_string();
        if by.is_empty() {
            return Err(Error::Config("a sign-off has to say who made it".into()));
        }
        let hash = (!withdraw).then(|| signed_hash(&note.body));
        if withdraw && !self.sign_offs()?.contains_key(&note.front.id.to_string()) {
            return Err(Error::Config(format!(
                "{name} carries no sign-off to withdraw"
            )));
        }
        let w = self.writers(dry_run)?;
        let policy = w.policy.get();
        let row = policy.audit().record_raw(
            &self.actor.to_string(),
            NOTE_SIGNOFF,
            format!("note:{}", note.front.id),
            json!({
                "name": note.front.name,
                "ring": note.front.ring,
                "by": by,
                "blake3": hash,
                "withdrawn": withdraw,
            }),
        )?;
        Ok(SignOffReport {
            name,
            ring: note.front.ring,
            by,
            at: row.ts,
            blake3: hash,
            withdrawn: withdraw,
            dry_run,
            audit_preview: w.policy.preview(),
        })
    }

    /// The standing sign-off per note id, from the audit log. Oldest row first, the last one
    /// for a note wins, a withdrawal removes it. Rows by anybody but the operator are not
    /// sign-offs, whatever they say.
    fn sign_offs(&self) -> Result<HashMap<String, Signed>> {
        let mut out = HashMap::new();
        for r in self.audit_sink.read_exact_action(NOTE_SIGNOFF)? {
            if r.actor != "operator" && r.actor != "cli" {
                continue;
            }
            let Some(id) = r.subject.strip_prefix("note:") else {
                continue;
            };
            match (
                r.detail.get("blake3").and_then(|v| v.as_str()),
                r.detail.get("by").and_then(|v| v.as_str()),
            ) {
                (Some(hash), Some(by)) => {
                    out.insert(
                        id.to_string(),
                        Signed {
                            by: by.to_string(),
                            at: r.ts,
                            hash: hash.to_string(),
                        },
                    );
                }
                _ => {
                    out.remove(id);
                }
            }
        }
        Ok(out)
    }

    /// The sign-off a note carries now: the last one recorded, if its body is still the one
    /// that was signed.
    pub fn signoff_of(&self, note: &Note) -> Option<SignOff> {
        let s = self.sign_offs().ok()?.remove(&note.front.id.to_string())?;
        (s.hash == signed_hash(&note.body)).then_some(SignOff { by: s.by, at: s.at })
    }

    /// Put the standing sign-off on every hit whose note still reads as signed. Never fails
    /// a recall: an audit log that cannot be read leaves the hits unmarked, which is the same
    /// as "nobody checked this".
    pub(crate) fn annotate_signoffs(&self, hits: &mut [Hit]) {
        if hits.is_empty() {
            return;
        }
        let Ok(signed) = self.sign_offs() else {
            return;
        };
        if signed.is_empty() {
            return;
        }
        let mut memo: HashMap<NoteId, Option<SignOff>> = HashMap::new();
        for h in hits.iter_mut() {
            let mark = memo.entry(h.note_id).or_insert_with(|| {
                let s = signed.get(&h.note_id.to_string())?;
                let note = self.store.read(&h.note_name).ok()?;
                (note.front.id == h.note_id && s.hash == signed_hash(&note.body)).then(|| SignOff {
                    by: s.by.clone(),
                    at: s.at,
                })
            });
            h.signed_off = mark.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(dir: &Path) -> PathBuf {
        let store = dir.join(DEFAULT_STORE_DIR);
        App::init(&store, &Actor::Operator).unwrap();
        store
    }

    fn write(app: &App, name: &str, body: &str) {
        app.write(WriteRequest {
            ring: Ring::Knowledge,
            kind: NoteKind::Knowledge,
            name: name.into(),
            body: body.into(),
            tags: Vec::new(),
            bereich: None,
            retention: None,
            force: false,
            choice: None,
            expected_updated: None,
            supersedes: None,
            valid_from: None,
            invalid_at: None,
            arriving: None,
            anchors: Anchoring::default(),
            dry_run: false,
        })
        .unwrap();
    }

    fn top(app: &App, q: &str) -> Hit {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(app.recall(q, &RecallRequest::default()))
            .unwrap()
            .hits
            .remove(0)
    }

    /// Signed, shown, recorded with the body's hash; gone the moment the body changes.
    #[test]
    fn a_signoff_shows_until_the_content_changes() {
        let dir = tempfile::tempdir().unwrap();
        let app = App::open(Some(&store(dir.path())), Actor::Operator).unwrap();
        write(&app, "checked-fact", "Postgres 18 moved PGDATA.");
        assert_eq!(top(&app, "postgres pgdata").signed_off, None);

        let r = app
            .sign_off("checked-fact", "anna@example.org", false, false)
            .unwrap();
        let hit = top(&app, "postgres pgdata");
        assert_eq!(
            hit.signed_off,
            Some(SignOff {
                by: "anna@example.org".into(),
                at: r.at
            })
        );
        let rows = app.audit_sink.read_exact_action(NOTE_SIGNOFF).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].actor, "operator");
        assert_eq!(rows[0].detail["by"], "anna@example.org");
        assert_eq!(
            rows[0].detail["blake3"],
            signed_hash("Postgres 18 moved PGDATA.")
        );
        assert!(rows[0].chain_hash().is_some(), "the row is in the chain");
        let note = app.store().read("checked-fact").unwrap();
        assert!(app.signoff_of(&note).is_some());

        write(&app, "checked-fact", "Postgres 18 moved PGDATA, and more.");
        assert_eq!(top(&app, "postgres pgdata").signed_off, None);
        let note = app.store().read("checked-fact").unwrap();
        assert_eq!(app.signoff_of(&note), None);
    }

    /// A withdrawal takes the mark off.
    #[test]
    fn a_withdrawn_signoff_is_gone() {
        let dir = tempfile::tempdir().unwrap();
        let app = App::open(Some(&store(dir.path())), Actor::Operator).unwrap();
        write(&app, "fact", "A checked fact.");
        app.sign_off("fact", "anna", false, false).unwrap();
        app.sign_off("fact", "anna", true, false).unwrap();
        assert_eq!(top(&app, "checked fact").signed_off, None);
        assert!(app.sign_off("fact", "anna", true, false).is_err());
    }

    /// Nobody but the operator signs: not an agent at the CLI, not MCP, not a hook, not a
    /// served store that cannot tell who is asking. And a row an agent got into the log by
    /// other means is not read as a sign-off.
    #[test]
    fn only_the_operator_signs_off() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        let op = App::open(Some(&store), Actor::Operator).unwrap();
        write(&op, "fact", "A checked fact.");
        for actor in [
            Actor::Agent("claude-code:abc".into()),
            Actor::Mcp,
            Actor::Hook("user-prompt-submit".into()),
        ] {
            let app = App::open(Some(&store), actor.clone()).unwrap();
            let e = app.sign_off("fact", "anna", false, false).unwrap_err();
            assert!(matches!(e, Error::PolicyRefusal { .. }), "{actor}: {e}");
        }
        let served = App::open(Some(&store), Actor::Operator).unwrap();
        served.refuse_resident_writes();
        assert!(matches!(
            served.sign_off("fact", "anna", false, false).unwrap_err(),
            Error::PolicyRefusal { .. }
        ));

        let note = op.store().read("fact").unwrap();
        op.policy
            .audit()
            .record_raw(
                "agent:claude-code:abc",
                NOTE_SIGNOFF,
                format!("note:{}", note.front.id),
                json!({ "by": "anna", "blake3": signed_hash(&note.body) }),
            )
            .unwrap();
        assert_eq!(top(&op, "checked fact").signed_off, None);
        assert!(
            op.audit_sink.read_exact_action(NOTE_SIGNOFF).unwrap().len() == 1,
            "only the forged row is there"
        );
    }

    /// A dry run records nothing.
    #[test]
    fn a_dry_run_signs_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let app = App::open(Some(&store(dir.path())), Actor::Operator).unwrap();
        write(&app, "fact", "A checked fact.");
        app.sign_off("fact", "anna", false, true).unwrap();
        assert_eq!(top(&app, "checked fact").signed_off, None);
    }
}
