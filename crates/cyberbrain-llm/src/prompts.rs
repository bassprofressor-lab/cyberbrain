//! Every prompt this crate sends, in one file, as text. Review them here.
//!
//! Conventions shared by all four:
//! - The system prompt fixes the role and demands a single JSON object and nothing else.
//! - The user prompt carries the material inside clearly delimited fences and repeats the
//!   JSON shape, because small local models follow the last instruction best.
//! - Material is inserted verbatim. It comes from the operator's own notes and is sent to
//!   the operator's own endpoint; there is no injection boundary to defend here beyond the
//!   fences, and the parsers in `tasks.rs` tolerate a model that talks around the JSON.

use crate::types::{ChatMessage, ChatRequest};

/// Task names as they appear in audit rows.
pub const TASK_SESSION_SUMMARY: &str = "session-summary";
pub const TASK_CONTRADICTION: &str = "contradiction-check";
pub const TASK_RING_TAGS: &str = "ring-tag-suggestion";
pub const TASK_SUPERSEDE: &str = "note-supersession";

const JSON_ONLY: &str = "Answer with exactly one JSON object and no other text: no prose before \
or after it, no Markdown code fence.";

// ---------------------------------------------------------------------------------------
// 1. Session summary
// ---------------------------------------------------------------------------------------

pub const SESSION_SUMMARY_SYSTEM: &str = "You write the memory note an AI coding agent will \
read at the start of its next session on this project. You are given the transcript of the \
session that just ended. Record what was done, what was decided and why, what broke, and what \
is still open. Be concrete: file paths, commands, error messages and numbers are more useful \
than adjectives. Do not invent anything that is not in the transcript. Do not include \
credentials, tokens or personal data even if they appear in the transcript; write \
\"[redacted]\" instead.";

pub fn session_summary(transcript: &str) -> ChatRequest {
    let user = format!(
        "Summarise the session transcript between the fences.\n\n\
         <<<TRANSCRIPT\n{transcript}\nTRANSCRIPT>>>\n\n\
         {JSON_ONLY} Shape:\n\
         {{\n  \
           \"title\": \"short kebab-case slug, 3 to 6 words\",\n  \
           \"summary\": \"Markdown body, 5 to 30 lines, past tense, concrete\",\n  \
           \"decisions\": [\"one line per decision, with the reason\"],\n  \
           \"open_items\": [\"one line per thing left undone or unresolved\"],\n  \
           \"tags\": [\"lowercase\", \"topic\", \"tags\"]\n\
         }}"
    );
    ChatRequest::new(vec![
        ChatMessage::system(SESSION_SUMMARY_SYSTEM),
        ChatMessage::user(user),
    ])
    .with_task(TASK_SESSION_SUMMARY)
}

// ---------------------------------------------------------------------------------------
// 2. Contradiction between two blocks
// ---------------------------------------------------------------------------------------

pub const CONTRADICTION_SYSTEM: &str = "You check whether two short passages from a project's \
notes contradict each other. A contradiction means both cannot be true of the same thing at \
the same time: different values for the same setting, opposite instructions for the same \
situation, incompatible facts about the same file or system. Different topics, different \
levels of detail, one being older, or one adding to the other are NOT contradictions. When in \
doubt, answer no: a false alarm costs the reader more than a miss.";

pub fn contradiction(
    a_citation: &str,
    a_text: &str,
    b_citation: &str,
    b_text: &str,
) -> ChatRequest {
    let user = format!(
        "Passage A (citation {a_citation}):\n<<<A\n{a_text}\nA>>>\n\n\
         Passage B (citation {b_citation}):\n<<<B\n{b_text}\nB>>>\n\n\
         Do A and B contradict each other? {JSON_ONLY} Shape:\n\
         {{\n  \
           \"contradictory\": true or false,\n  \
           \"confidence\": number from 0.0 to 1.0,\n  \
           \"reason\": \"one sentence quoting the conflicting parts, or why they are compatible\"\n\
         }}"
    );
    ChatRequest::new(vec![
        ChatMessage::system(CONTRADICTION_SYSTEM),
        ChatMessage::user(user),
    ])
    .with_task(TASK_CONTRADICTION)
}

/// One call over a whole result set (SPEC §16 keeps contradiction detection to a single
/// model call in v0.1). `blocks` is `(citation, text)`.
pub fn contradictions_among(blocks: &[(String, String)]) -> ChatRequest {
    let mut user = String::from(
        "Below are passages retrieved from a project's notes, each with a citation. Find every \
         pair that contradicts each other under the definition you were given.\n\n",
    );
    for (cit, text) in blocks {
        user.push_str(&format!("<<<{cit}\n{text}\n{cit}>>>\n\n"));
    }
    user.push_str(&format!(
        "{JSON_ONLY} Shape:\n\
         {{\n  \
           \"pairs\": [\n    \
             {{\"a\": \"citation\", \"b\": \"citation\", \"confidence\": 0.0 to 1.0, \
             \"reason\": \"one sentence\"}}\n  \
           ]\n\
         }}\n\
         An empty \"pairs\" list is the right answer when nothing contradicts."
    ));
    ChatRequest::new(vec![
        ChatMessage::system(CONTRADICTION_SYSTEM),
        ChatMessage::user(user),
    ])
    .with_task(TASK_CONTRADICTION)
}

// ---------------------------------------------------------------------------------------
// 3. Ring and tag suggestion
// ---------------------------------------------------------------------------------------

pub const RING_TAGS_SYSTEM: &str = "You file a new note into a trust-tiered project memory. \
Rings:\n\
0 = operator invariants: hard rules the operator wrote that override everything (you may \
not propose this ring; only the operator files here)\n\
1 = operating protocol and active handoff state (you may not propose this ring either)\n\
2 = curated project knowledge: verified facts, decisions, lessons, bug write-ups\n\
3 = session records and observations: what happened, not yet distilled\n\
4 = imported or unverified external material\n\
Kinds: knowledge, bug, lesson, decision, reference, session.\n\
Propose ring 2 only for material that reads as verified and durable; a session log is ring \
3; pasted documentation or third-party text is ring 4.";

pub fn ring_and_tags(name: &str, body: &str, existing_tags: &[String]) -> ChatRequest {
    let vocab = if existing_tags.is_empty() {
        String::from("There is no existing tag vocabulary; invent short lowercase tags.")
    } else {
        format!(
            "Prefer tags from the existing vocabulary where they fit: {}",
            existing_tags.join(", ")
        )
    };
    let user = format!(
        "Note name: {name}\n\n<<<NOTE\n{body}\nNOTE>>>\n\n{vocab}\n\n\
         {JSON_ONLY} Shape:\n\
         {{\n  \
           \"ring\": 2, 3 or 4,\n  \
           \"kind\": \"knowledge\" | \"bug\" | \"lesson\" | \"decision\" | \"reference\" | \"session\",\n  \
           \"tags\": [\"two\", \"to\", \"six\", \"tags\"],\n  \
           \"reason\": \"one sentence\"\n\
         }}"
    );
    ChatRequest::new(vec![
        ChatMessage::system(RING_TAGS_SYSTEM),
        ChatMessage::user(user),
    ])
    .with_task(TASK_RING_TAGS)
}

// ---------------------------------------------------------------------------------------
// 4. Supersession: rewrite a note in light of new evidence
// ---------------------------------------------------------------------------------------

pub const SUPERSEDE_SYSTEM: &str = "You maintain a project's notes. You are given an existing \
note and a piece of new evidence that may supersede part of it. Rewrite the note so that it is \
true given the new evidence. Rules: change only what the evidence contradicts or extends; keep \
every other sentence exactly as it was; keep the Markdown structure, headings and [[links]]; \
where a statement is superseded, replace it rather than appending a correction; do not add \
opinions; do not drop information the evidence does not touch. If the evidence changes \
nothing, return the note unchanged and say so.";

pub fn supersede(note_name: &str, note_body: &str, evidence: &str) -> ChatRequest {
    let user = format!(
        "Existing note ({note_name}):\n<<<NOTE\n{note_body}\nNOTE>>>\n\n\
         New evidence:\n<<<EVIDENCE\n{evidence}\nEVIDENCE>>>\n\n\
         {JSON_ONLY} Shape:\n\
         {{\n  \
           \"changed\": true or false,\n  \
           \"body\": \"the full rewritten note body in Markdown, or the original if unchanged\",\n  \
           \"superseded\": [\"each original sentence or line you replaced, verbatim\"],\n  \
           \"rationale\": \"one or two sentences\"\n\
         }}"
    );
    ChatRequest::new(vec![
        ChatMessage::system(SUPERSEDE_SYSTEM),
        ChatMessage::user(user),
    ])
    .with_task(TASK_SUPERSEDE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_prompt_is_system_plus_user_and_demands_json() {
        let reqs = [
            session_summary("t"),
            contradiction("r2-a", "x", "r3-b", "y"),
            contradictions_among(&[("r2-a".into(), "x".into())]),
            ring_and_tags("n", "b", &[]),
            supersede("n", "b", "e"),
        ];
        for r in reqs {
            assert_eq!(r.messages.len(), 2);
            assert_eq!(r.messages[0].role, crate::types::Role::System);
            assert_eq!(r.messages[1].role, crate::types::Role::User);
            assert!(r.messages[1].content.contains("exactly one JSON object"));
            assert!(r.task.is_some());
        }
    }

    #[test]
    fn material_is_fenced_verbatim() {
        let r = contradiction(
            "r2-aaaaaaaaaa",
            "Port is 5432.",
            "r3-bbbbbbbbbb",
            "Port is 5433.",
        );
        let u = &r.messages[1].content;
        assert!(u.contains("<<<A\nPort is 5432.\nA>>>"));
        assert!(u.contains("citation r3-bbbbbbbbbb"));
    }

    #[test]
    fn ring_prompt_forbids_resident_rings() {
        assert!(RING_TAGS_SYSTEM.contains("may not propose this ring"));
        let r = ring_and_tags("n", "b", &["postgres".into()]);
        assert!(r.messages[1].content.contains("postgres"));
    }
}
