//! Where a note's content came from, as tags.
//!
//! A note's ring says how much it is trusted relative to other notes; it does not say where
//! the text came from. An agent that files an e-mail, a web page or another agent's output
//! into ring 2 produces a note that reads exactly like one the operator wrote, and the next
//! agent that recalls it follows it. That is the attack the memory-protection offer is about
//! (Bauplan 2026-10-02, C1).
//!
//! **Tags, not frontmatter fields.** `Frontmatter` rejects unknown keys, so a new field would
//! make every older binary refuse the note. Tags already travel everywhere a note goes: the
//! index, the hub, export. The vocabulary:
//!
//! - `trust:untrusted` — the content came from outside (mail, web, another agent's output) and
//!   is data, not instruction. `trust:trusted` may be written for clarity and means the same
//!   as no `trust:` tag.
//! - `src:<kind>[:<id>]` — what it came from, e.g. `src:mail:4f2a`, `src:web`,
//!   `src:agent:seo`. No personal data: tags are indexed and shown unredacted, and the PII
//!   gate refuses a tag with an address in it.
//!
//! What a `trust:untrusted` note means at write time is decided in `App::write`.

/// The tag that marks content from outside.
pub const UNTRUSTED: &str = "trust:untrusted";

const TRUST_PREFIX: &str = "trust:";
const SRC_PREFIX: &str = "src:";

/// `Err(reason)` when the provenance tags among `tags` contradict themselves or are empty.
/// Tags without these prefixes are not looked at.
pub fn validate(tags: &[String]) -> Result<(), String> {
    let mut trust: Option<&str> = None;
    for t in tags {
        if let Some(v) = t.strip_prefix(TRUST_PREFIX) {
            if v != "trusted" && v != "untrusted" {
                return Err(format!(
                    "tag `{t}`: trust is `trust:trusted` or `trust:untrusted`"
                ));
            }
            if let Some(prev) = trust
                && prev != v
            {
                return Err("a note cannot be both `trust:trusted` and `trust:untrusted`".into());
            }
            trust = Some(v);
        } else if let Some(v) = t.strip_prefix(SRC_PREFIX)
            && v.trim_matches(':').is_empty()
        {
            return Err(format!(
                "tag `{t}`: name the source, e.g. `src:mail`, `src:web`, `src:agent:seo`"
            ));
        }
    }
    Ok(())
}

/// Whether the tags mark the content as from outside.
pub fn is_untrusted(tags: &[String]) -> bool {
    tags.iter().any(|t| t == UNTRUSTED)
}

/// For an untrusted note, what it came from: the first `src:` tag without its prefix, or
/// `unspecified`. `None` for a note that is not marked untrusted.
pub fn untrusted_source(tags: &[String]) -> Option<String> {
    if !is_untrusted(tags) {
        return None;
    }
    Some(
        tags.iter()
            .find_map(|t| t.strip_prefix(SRC_PREFIX))
            .unwrap_or("unspecified")
            .to_string(),
    )
}

/// Mark `tags` as content from `client`, for a client the operator named untrusted (C4):
/// `trust:trusted` is dropped, `trust:untrusted` added, and `src:<client>` added unless the
/// client named a source of its own (which stays: it is more precise, and untrusted anyway).
pub fn stamp_untrusted(tags: &mut Vec<String>, client: &str) {
    tags.retain(|t| t != "trust:trusted");
    if !is_untrusted(tags) {
        tags.push(UNTRUSTED.to_string());
    }
    if !tags.iter().any(|t| t.starts_with(SRC_PREFIX)) {
        tags.push(format!("{SRC_PREFIX}{client}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(t: &[&str]) -> Vec<String> {
        t.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_vocabulary_is_checked_and_nothing_else_is() {
        assert!(validate(&v(&["trust:untrusted", "src:mail:4f2a", "anything"])).is_ok());
        assert!(validate(&v(&["trust:trusted"])).is_ok());
        assert!(validate(&v(&["trust:maybe"])).is_err());
        assert!(validate(&v(&["trust:trusted", "trust:untrusted"])).is_err());
        assert!(validate(&v(&["src:"])).is_err());
        assert!(validate(&v(&["src::"])).is_err());
        assert!(validate(&v(&["source:x", "trusty"])).is_ok());
    }

    #[test]
    fn a_named_client_is_stamped_untrusted_whatever_it_sends() {
        let mut t = v(&["trust:trusted", "x"]);
        stamp_untrusted(&mut t, "agent:seo");
        assert_eq!(t, v(&["x", "trust:untrusted", "src:agent:seo"]));
        let mut t = v(&["src:web"]);
        stamp_untrusted(&mut t, "mcp");
        assert_eq!(t, v(&["src:web", "trust:untrusted"]));
        assert!(validate(&t).is_ok());
    }

    #[test]
    fn the_source_is_named_only_for_untrusted_notes() {
        assert_eq!(untrusted_source(&v(&["src:web"])), None);
        assert_eq!(
            untrusted_source(&v(&["trust:untrusted", "src:mail:4f2a"])).as_deref(),
            Some("mail:4f2a")
        );
        assert_eq!(
            untrusted_source(&v(&["trust:untrusted"])).as_deref(),
            Some("unspecified")
        );
    }
}
