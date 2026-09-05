//! The four features SPEC §11 names, as typed functions. Each one:
//! - builds its prompt from `prompts.rs`,
//! - makes one call,
//! - parses the JSON the model was asked for, tolerating prose and code fences around it,
//! - returns `Feature<T>`: the value, or a `Degraded` with a caveat sentence.
//!
//! Nothing here blocks on the endpoint beyond the client's timeout, and nothing here has a
//! second provider to try.

use cyberbrain_core::{Conflict, Error, Hit, NoteKind, Ring};
use serde::Deserialize;

use crate::client::LlmClient;
use crate::degrade::{Degraded, Feature, degrade};
use crate::prompts;

/// Inputs above this many characters are cut before being sent, and the result says so.
/// Local context windows are small; a truncated summary with a flag beats a 400 from the
/// server or a summary of the tail only.
pub const MAX_INPUT_CHARS: usize = 60_000;

const F_SUMMARY: &str = "session summary";
const F_CONTRADICTION: &str = "contradiction check";
const F_RING_TAGS: &str = "ring and tag suggestion";
const F_SUPERSEDE: &str = "note supersession";

// ---------------------------------------------------------------------------------------
// 1. Session summary
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSummary {
    /// kebab-case slug proposed by the model. Callers must still enforce uniqueness.
    pub title: String,
    /// Markdown body for a ring-3 `session` note.
    pub summary: String,
    pub decisions: Vec<String>,
    pub open_items: Vec<String>,
    pub tags: Vec<String>,
    /// True if the transcript was cut at `MAX_INPUT_CHARS` before summarising.
    pub input_truncated: bool,
}

#[derive(Deserialize)]
struct SummaryWire {
    title: String,
    summary: String,
    #[serde(default)]
    decisions: Vec<String>,
    #[serde(default)]
    open_items: Vec<String>,
    #[serde(default)]
    tags: Vec<String>,
}

pub async fn summarize_session(client: &LlmClient, transcript: &str) -> Feature<SessionSummary> {
    let (input, truncated) = truncate(transcript);
    let req = prompts::session_summary(input);
    let resp = client.chat(&req).await.map_err(degrade(F_SUMMARY))?;
    let w: SummaryWire = parse_json(&resp.content).map_err(degrade(F_SUMMARY))?;
    Ok(SessionSummary {
        title: slugify(&w.title),
        summary: w.summary.trim().to_string(),
        decisions: w.decisions,
        open_items: w.open_items,
        tags: normalise_tags(w.tags),
        input_truncated: truncated,
    })
}

// ---------------------------------------------------------------------------------------
// 2. Contradiction detection
// ---------------------------------------------------------------------------------------

/// A block as the contradiction check needs it. Built from a `Hit` or by hand.
#[derive(Debug, Clone)]
pub struct BlockRef {
    pub citation: String,
    pub ring: Ring,
    pub text: String,
}

impl From<&Hit> for BlockRef {
    fn from(h: &Hit) -> Self {
        BlockRef {
            citation: h.citation.clone(),
            ring: h.ring,
            text: h.text.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContradictionVerdict {
    pub contradictory: bool,
    /// 0.0 to 1.0 as reported by the model, clamped.
    pub confidence: f32,
    pub reason: String,
}

#[derive(Deserialize)]
struct VerdictWire {
    contradictory: bool,
    #[serde(default)]
    confidence: f32,
    #[serde(default)]
    reason: String,
}

/// Ask whether two blocks contradict each other. One call.
pub async fn detect_contradiction(
    client: &LlmClient,
    a: &BlockRef,
    b: &BlockRef,
) -> Feature<ContradictionVerdict> {
    let req = prompts::contradiction(&a.citation, &a.text, &b.citation, &b.text);
    let resp = client.chat(&req).await.map_err(degrade(F_CONTRADICTION))?;
    let w: VerdictWire = parse_json(&resp.content).map_err(degrade(F_CONTRADICTION))?;
    Ok(ContradictionVerdict {
        contradictory: w.contradictory,
        confidence: w.confidence.clamp(0.0, 1.0),
        reason: w.reason.trim().to_string(),
    })
}

/// Turn a verdict on two blocks into the `Conflict` a `RecallResult` carries. The lower
/// ring wins (SPEC §3.2). Returns `None` if the verdict is negative, and also if both blocks
/// are in the same ring: the spec defines conflicts between different rings only and has no
/// precedence rule inside a ring, so we do not invent one.
pub fn conflict_between(a: &BlockRef, b: &BlockRef, v: &ContradictionVerdict) -> Option<Conflict> {
    if !v.contradictory || a.ring == b.ring {
        return None;
    }
    let (winner, loser) = if a.ring < b.ring { (a, b) } else { (b, a) };
    Some(Conflict {
        winner: winner.citation.clone(),
        loser: loser.citation.clone(),
        reason: format!(
            "{} ({}) overrides {} ({}), confidence {:.2}: {}",
            winner.citation, winner.ring, loser.citation, loser.ring, v.confidence, v.reason
        ),
    })
}

#[derive(Deserialize)]
struct PairsWire {
    #[serde(default)]
    pairs: Vec<PairWire>,
}

#[derive(Deserialize)]
struct PairWire {
    a: String,
    b: String,
    #[serde(default)]
    confidence: f32,
    #[serde(default)]
    reason: String,
}

/// What the recall path calls: one model call over the whole hit list. Returns the
/// conflicts to attach and the caveats to attach, never an error. Same-ring pairs the
/// model reports are dropped and named in a caveat rather than silently ignored.
pub async fn find_conflicts(client: &LlmClient, hits: &[Hit]) -> (Vec<Conflict>, Vec<String>) {
    let mut caveats = Vec::new();
    if let Some(c) = client.waiver_caveat() {
        caveats.push(c);
    }
    let rings: std::collections::BTreeSet<Ring> = hits.iter().map(|h| h.ring).collect();
    if rings.len() < 2 {
        caveats.push(format!(
            "{F_CONTRADICTION} skipped: all {} hits are in one ring, and conflicts are defined \
             across rings",
            hits.len()
        ));
        return (Vec::new(), caveats);
    }

    let blocks: Vec<(String, String)> = hits
        .iter()
        .map(|h| (h.citation.clone(), h.text.clone()))
        .collect();
    let req = prompts::contradictions_among(&blocks);
    let parsed: Result<PairsWire, Degraded> = async {
        let resp = client.chat(&req).await.map_err(degrade(F_CONTRADICTION))?;
        parse_json(&resp.content).map_err(degrade(F_CONTRADICTION))
    }
    .await;
    let w = match parsed {
        Ok(w) => w,
        Err(d) => {
            caveats.push(d.caveat);
            return (Vec::new(), caveats);
        }
    };

    let mut conflicts = Vec::new();
    let mut same_ring = Vec::new();
    let mut unknown = Vec::new();
    for p in w.pairs {
        let (Some(a), Some(b)) = (
            hits.iter().find(|h| h.citation == p.a),
            hits.iter().find(|h| h.citation == p.b),
        ) else {
            unknown.push(format!("{}/{}", p.a, p.b));
            continue;
        };
        let (a, b) = (BlockRef::from(a), BlockRef::from(b));
        let v = ContradictionVerdict {
            contradictory: true,
            confidence: p.confidence.clamp(0.0, 1.0),
            reason: p.reason.trim().to_string(),
        };
        match conflict_between(&a, &b, &v) {
            Some(c) => conflicts.push(c),
            None => same_ring.push(format!("{} vs {} ({})", a.citation, b.citation, a.ring)),
        }
    }
    if !same_ring.is_empty() {
        caveats.push(format!(
            "{F_CONTRADICTION}: model flagged same-ring pairs with no precedence rule, left \
             unresolved: {}",
            same_ring.join(", ")
        ));
    }
    if !unknown.is_empty() {
        caveats.push(format!(
            "{F_CONTRADICTION}: model named citations that are not in the result set, ignored: {}",
            unknown.join(", ")
        ));
    }
    (conflicts, caveats)
}

// ---------------------------------------------------------------------------------------
// 3. Ring and tag suggestion
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RingTagSuggestion {
    pub ring: Ring,
    pub kind: Option<NoteKind>,
    pub tags: Vec<String>,
    pub reason: String,
    /// The model proposed ring 0 or 1 and was moved to ring 2. Those rings are the
    /// operator's alone (SPEC §3.2); the suggestion says so rather than obeying.
    pub demoted_from_resident: bool,
}

#[derive(Deserialize)]
struct RingTagWire {
    ring: u8,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    reason: String,
}

pub async fn suggest_ring_and_tags(
    client: &LlmClient,
    name: &str,
    body: &str,
    existing_tags: &[String],
) -> Feature<RingTagSuggestion> {
    let (input, _) = truncate(body);
    let req = prompts::ring_and_tags(name, input, existing_tags);
    let resp = client.chat(&req).await.map_err(degrade(F_RING_TAGS))?;
    let w: RingTagWire = parse_json(&resp.content).map_err(degrade(F_RING_TAGS))?;

    let proposed = Ring::try_from(w.ring).map_err(|e| {
        Degraded::new(
            F_RING_TAGS,
            Error::Llm(format!("model proposed an invalid ring: {e}")),
        )
    })?;
    let (ring, demoted) = if proposed.is_resident() {
        (Ring::Knowledge, true)
    } else {
        (proposed, false)
    };
    let mut reason = w.reason.trim().to_string();
    if demoted {
        reason = format!(
            "model proposed {proposed}, which only the operator may file into; suggesting r2 \
             instead. Model's reason: {reason}"
        );
    }
    let kind = w.kind.as_deref().and_then(parse_kind);
    Ok(RingTagSuggestion {
        ring,
        kind,
        tags: normalise_tags(w.tags),
        reason,
        demoted_from_resident: demoted,
    })
}

// ---------------------------------------------------------------------------------------
// 4. Supersession
// ---------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Supersession {
    /// True if `body` differs from the original.
    pub changed: bool,
    /// Full rewritten body. Equal to the input when `changed` is false.
    pub body: String,
    /// Original passages the model says it replaced.
    pub superseded: Vec<String>,
    pub rationale: String,
}

#[derive(Deserialize)]
struct SupersedeWire {
    #[serde(default)]
    changed: Option<bool>,
    body: String,
    #[serde(default)]
    superseded: Vec<String>,
    #[serde(default)]
    rationale: String,
}

/// Rewrite `note_body` in light of `evidence`. The caller shows the diff and writes the
/// file; this function touches no storage. `changed` is computed from the text, not taken
/// from the model, so a model that says "unchanged" while changing the text is caught.
pub async fn supersede_note(
    client: &LlmClient,
    note_name: &str,
    note_body: &str,
    evidence: &str,
) -> Feature<Supersession> {
    let req = prompts::supersede(note_name, note_body, evidence);
    let resp = client.chat(&req).await.map_err(degrade(F_SUPERSEDE))?;
    let w: SupersedeWire = parse_json(&resp.content).map_err(degrade(F_SUPERSEDE))?;
    let body = w.body.trim_end().to_string();
    let changed = body != note_body.trim_end();
    let mut rationale = w.rationale.trim().to_string();
    if w.changed == Some(false) && changed {
        rationale =
            format!("model reported no change but the text differs; review the diff. {rationale}");
    }
    if body.is_empty() {
        return Err(Degraded::new(
            F_SUPERSEDE,
            Error::Llm("model returned an empty body; refusing to propose erasing the note".into()),
        ));
    }
    Ok(Supersession {
        changed,
        body,
        superseded: w.superseded,
        rationale,
    })
}

// ---------------------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------------------

fn truncate(s: &str) -> (&str, bool) {
    if s.len() <= MAX_INPUT_CHARS {
        return (s, false);
    }
    let mut end = MAX_INPUT_CHARS;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    (&s[..end], true)
}

/// Find the JSON object in a model reply: strip code fences, take from the first `{` to
/// its matching `}`. Small models often add a sentence before or after.
pub(crate) fn parse_json<T: for<'de> Deserialize<'de>>(reply: &str) -> Result<T, Error> {
    let text = reply.trim();
    let text = text
        .strip_prefix("```json")
        .or_else(|| text.strip_prefix("```"))
        .map(|t| t.trim_end_matches("```"))
        .unwrap_or(text)
        .trim();
    let start = text.find('{').ok_or_else(|| {
        Error::Llm(format!(
            "model reply contained no JSON object: {:?}",
            head(text)
        ))
    })?;
    // Walk to the matching brace, honouring strings.
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    let mut in_str = false;
    let mut esc = false;
    let mut end = None;
    for (i, &c) in bytes.iter().enumerate().skip(start) {
        if in_str {
            if esc {
                esc = false;
            } else if c == b'\\' {
                esc = true;
            } else if c == b'"' {
                in_str = false;
            }
            continue;
        }
        match c {
            b'"' => in_str = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(i + 1);
                    break;
                }
            }
            _ => {}
        }
    }
    let end = end.ok_or_else(|| {
        Error::Llm(format!(
            "model reply has an unterminated JSON object: {:?}",
            head(text)
        ))
    })?;
    serde_json::from_str(&text[start..end])
        .map_err(|e| Error::Llm(format!("model reply is not the expected JSON shape: {e}")))
}

fn head(s: &str) -> String {
    s.chars().take(80).collect()
}

fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in s.trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    out.trim_end_matches('-').to_string()
}

fn normalise_tags(tags: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = tags
        .iter()
        .map(|t| slugify(t))
        .filter(|t| !t.is_empty())
        .collect();
    out.dedup();
    let mut seen = std::collections::HashSet::new();
    out.retain(|t| seen.insert(t.clone()));
    out
}

fn parse_kind(s: &str) -> Option<NoteKind> {
    Some(match s.trim().to_ascii_lowercase().as_str() {
        "knowledge" => NoteKind::Knowledge,
        "bug" => NoteKind::Bug,
        "lesson" => NoteKind::Lesson,
        "decision" => NoteKind::Decision,
        "reference" => NoteKind::Reference,
        "session" => NoteKind::Session,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::MemoryAuditSink;
    use crate::client::LlmConfig;
    use crate::mock::{MockResponse, MockServer};
    use std::time::Duration;

    fn reply(content: &str) -> String {
        serde_json::json!({
            "choices": [{"message": {"role": "assistant", "content": content}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
        })
        .to_string()
    }

    /// Permits everything; the real gate lives in `cyberbrain-policy`, off-limits here.
    #[derive(Debug)]
    struct OpenGate;

    impl cyberbrain_core::EgressGate for OpenGate {
        fn permit(&self, _p: cyberbrain_core::EgressPurpose, _d: &str) -> cyberbrain_core::Result<()> {
            Ok(())
        }
    }

    fn open_gate() -> std::sync::Arc<dyn cyberbrain_core::EgressGate> {
        std::sync::Arc::new(OpenGate)
    }

    async fn client_for(server: &MockServer) -> LlmClient {
        LlmClient::connect(
            LlmConfig {
                base_url: server.base_url("/v1"),
                model: "m".into(),
                timeout: Duration::from_secs(5),
                ..Default::default()
            },
            MemoryAuditSink::new(),
        
            open_gate(),
        )
        .await
        .unwrap()
    }

    fn hit(cit: &str, ring: Ring, text: &str) -> Hit {
        Hit {
            citation: cit.into(),
            note_id: cyberbrain_core::NoteId::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV").unwrap(),
            note_name: "n".into(),
            ring,
            score: 1.0,
            text: text.into(),
        }
    }

    #[test]
    fn json_extraction_tolerates_prose_and_fences() {
        #[derive(Deserialize, PartialEq, Debug)]
        struct T {
            a: u32,
        }
        assert_eq!(parse_json::<T>(r#"{"a":1}"#).unwrap(), T { a: 1 });
        assert_eq!(
            parse_json::<T>("Sure! Here you go:\n{\"a\": 2}\nHope that helps.").unwrap(),
            T { a: 2 }
        );
        assert_eq!(
            parse_json::<T>("```json\n{\"a\":3}\n```").unwrap(),
            T { a: 3 }
        );
        assert_eq!(
            parse_json::<T>(r#"{"a":4,"s":"has } brace \" and \\"}"#).unwrap(),
            T { a: 4 }
        );
        assert!(parse_json::<T>("no json here").is_err());
        assert!(parse_json::<T>("{\"a\": 1").is_err());
        assert!(parse_json::<T>("{\"b\": 1}").is_err());
    }

    #[test]
    fn slug_and_tags() {
        assert_eq!(slugify("  PG 18 moves PGDATA! "), "pg-18-moves-pgdata");
        assert_eq!(
            normalise_tags(vec![
                "Postgres".into(),
                "postgres".into(),
                "".into(),
                "docker/compose".into()
            ]),
            vec!["postgres", "docker-compose"]
        );
    }

    #[test]
    fn conflict_rules() {
        let a = BlockRef {
            citation: "r3-aaaaaaaaaa".into(),
            ring: Ring::Session,
            text: "x".into(),
        };
        let b = BlockRef {
            citation: "r1-bbbbbbbbbb".into(),
            ring: Ring::Protocol,
            text: "y".into(),
        };
        let yes = ContradictionVerdict {
            contradictory: true,
            confidence: 0.9,
            reason: "differ".into(),
        };
        let no = ContradictionVerdict {
            contradictory: false,
            confidence: 0.9,
            reason: "same".into(),
        };
        let c = conflict_between(&a, &b, &yes).unwrap();
        assert_eq!(
            c.winner, "r1-bbbbbbbbbb",
            "lower ring wins regardless of argument order"
        );
        assert_eq!(c.loser, "r3-aaaaaaaaaa");
        assert!(c.reason.contains("0.90"));
        assert!(conflict_between(&a, &b, &no).is_none());
        let a2 = BlockRef {
            ring: Ring::Protocol,
            ..a.clone()
        };
        assert!(
            conflict_between(&a2, &b, &yes).is_none(),
            "same ring: no precedence rule"
        );
    }

    #[tokio::test]
    async fn summary_round_trip_and_truncation_flag() {
        let server = MockServer::start(|req| {
            let body: serde_json::Value = serde_json::from_slice(&req.body).unwrap();
            let user = body["messages"][1]["content"].as_str().unwrap();
            assert!(user.contains("<<<TRANSCRIPT"));
            assert!(user.len() < MAX_INPUT_CHARS + 2000);
            MockResponse::json(200, &reply(r#"{"title":"Fix PG data dir","summary":"Did things.","decisions":["a"],"open_items":[],"tags":["Postgres","pg"]}"#))
        })
        .await;
        let client = client_for(&server).await;
        let long = "x".repeat(MAX_INPUT_CHARS + 10);
        let s = summarize_session(&client, &long).await.unwrap();
        assert_eq!(s.title, "fix-pg-data-dir");
        assert_eq!(s.tags, vec!["postgres", "pg"]);
        assert!(s.input_truncated);
        let s = summarize_session(&client, "short").await.unwrap();
        assert!(!s.input_truncated);
    }

    #[tokio::test]
    async fn contradiction_pairwise() {
        let server = MockServer::start(|_| {
            MockResponse::json(
                200,
                &reply(r#"{"contradictory": true, "confidence": 1.7, "reason": "5432 vs 5433"}"#),
            )
        })
        .await;
        let client = client_for(&server).await;
        let a = BlockRef {
            citation: "r2-aaaaaaaaaa".into(),
            ring: Ring::Knowledge,
            text: "port 5432".into(),
        };
        let b = BlockRef {
            citation: "r3-bbbbbbbbbb".into(),
            ring: Ring::Session,
            text: "port 5433".into(),
        };
        let v = detect_contradiction(&client, &a, &b).await.unwrap();
        assert!(v.contradictory);
        assert_eq!(v.confidence, 1.0, "clamped");
        assert_eq!(
            conflict_between(&a, &b, &v).unwrap().winner,
            "r2-aaaaaaaaaa"
        );
    }

    #[tokio::test]
    async fn find_conflicts_single_call_with_caveats() {
        let server = MockServer::start(|_| {
            MockResponse::json(200, &reply(
                r#"{"pairs":[{"a":"r2-aaaaaaaaaa","b":"r3-bbbbbbbbbb","confidence":0.8,"reason":"port"},
                             {"a":"r3-bbbbbbbbbb","b":"r3-cccccccccc","confidence":0.5,"reason":"same ring"},
                             {"a":"r2-aaaaaaaaaa","b":"r9-nope","confidence":0.5,"reason":"ghost"}]}"#,
            ))
        })
        .await;
        let client = client_for(&server).await;
        let hits = vec![
            hit("r2-aaaaaaaaaa", Ring::Knowledge, "port 5432"),
            hit("r3-bbbbbbbbbb", Ring::Session, "port 5433"),
            hit("r3-cccccccccc", Ring::Session, "port 5434"),
        ];
        let (conflicts, caveats) = find_conflicts(&client, &hits).await;
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].winner, "r2-aaaaaaaaaa");
        assert_eq!(caveats.len(), 2, "{caveats:?}");
        assert!(caveats[0].contains("same-ring"));
        assert!(caveats[1].contains("r9-nope"));
    }

    #[tokio::test]
    async fn find_conflicts_skips_single_ring_and_says_so() {
        let server = MockServer::start(|_| panic!("no call for a single-ring result set")).await;
        let client = client_for(&server).await;
        let hits = vec![
            hit("r2-aaaaaaaaaa", Ring::Knowledge, "a"),
            hit("r2-bbbbbbbbbb", Ring::Knowledge, "b"),
        ];
        let (c, caveats) = find_conflicts(&client, &hits).await;
        assert!(c.is_empty());
        assert_eq!(caveats.len(), 1);
        assert!(caveats[0].contains("skipped"), "{caveats:?}");
    }

    #[tokio::test]
    async fn find_conflicts_degrades_on_dead_endpoint_with_caveat() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        let client = LlmClient::connect(
            LlmConfig {
                base_url: format!("http://127.0.0.1:{port}/v1"),
                model: "m".into(),
                ..Default::default()
            },
            MemoryAuditSink::new(),
        
            open_gate(),
        )
        .await
        .unwrap();
        let hits = vec![
            hit("r2-aaaaaaaaaa", Ring::Knowledge, "a"),
            hit("r3-bbbbbbbbbb", Ring::Session, "b"),
        ];
        let (c, caveats) = find_conflicts(&client, &hits).await;
        assert!(c.is_empty());
        assert_eq!(caveats.len(), 1);
        assert!(
            caveats[0].starts_with("contradiction check skipped:"),
            "{caveats:?}"
        );
        assert!(caveats[0].contains("unreachable"), "{caveats:?}");
    }

    #[tokio::test]
    async fn ring_suggestion_demotes_resident_rings() {
        let server = MockServer::start(|_| {
            MockResponse::json(
                200,
                &reply(r#"{"ring": 0, "kind": "Decision", "tags": ["Infra"], "reason": "rule"}"#),
            )
        })
        .await;
        let client = client_for(&server).await;
        let s = suggest_ring_and_tags(&client, "n", "body", &[])
            .await
            .unwrap();
        assert_eq!(s.ring, Ring::Knowledge);
        assert!(s.demoted_from_resident);
        assert!(s.reason.contains("r0"));
        assert_eq!(s.kind, Some(NoteKind::Decision));
        assert_eq!(s.tags, vec!["infra"]);
    }

    #[tokio::test]
    async fn ring_suggestion_invalid_ring_degrades() {
        let server = MockServer::start(|_| MockResponse::json(200, &reply(r#"{"ring": 7}"#))).await;
        let client = client_for(&server).await;
        let d = suggest_ring_and_tags(&client, "n", "body", &[])
            .await
            .unwrap_err();
        assert!(d.caveat.contains("invalid ring"), "{}", d.caveat);
    }

    #[tokio::test]
    async fn supersession_computes_changed_from_text() {
        let server = MockServer::start(|_| {
            MockResponse::json(200, &reply(r#"{"changed": false, "body": "Port is 5433.\n", "superseded": ["Port is 5432."], "rationale": "r"}"#))
        })
        .await;
        let client = client_for(&server).await;
        let s = supersede_note(&client, "n", "Port is 5432.\n", "it is 5433")
            .await
            .unwrap();
        assert!(
            s.changed,
            "text differs even though the model said otherwise"
        );
        assert!(s.rationale.contains("review the diff"));
        assert_eq!(s.body, "Port is 5433.");
    }

    #[tokio::test]
    async fn supersession_refuses_empty_body() {
        let server =
            MockServer::start(|_| MockResponse::json(200, &reply(r#"{"body": ""}"#))).await;
        let client = client_for(&server).await;
        let d = supersede_note(&client, "n", "x", "e").await.unwrap_err();
        assert!(d.caveat.contains("empty body"));
    }

    #[tokio::test]
    async fn non_json_reply_degrades_with_caveat() {
        let server =
            MockServer::start(|_| MockResponse::json(200, &reply("I cannot help with that.")))
                .await;
        let client = client_for(&server).await;
        let d = summarize_session(&client, "t").await.unwrap_err();
        assert!(
            d.caveat.starts_with("session summary skipped:"),
            "{}",
            d.caveat
        );
        assert!(d.caveat.contains("no JSON object"));
    }
}
