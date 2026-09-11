//! `[[name]]` references in a note body (SPEC §3.1).
//!
//! A link names intent, not a fact: a link to a note that does not exist is valid, is
//! reported by `doctor` as dangling, and is never an error here. This module only finds
//! the references; resolution against the store is the caller's business.
//!
//! What is skipped: anything inside a fenced code block and anything inside an inline
//! code span, because `[[...]]` there is being shown, not used.
//!
//! What is accepted: `[[target]]`, `[[target|alias]]` and `[[target#heading]]` — the
//! target is the part before the first `|` or `#`, trimmed. The spec only shows the bare
//! form; the two extensions are the common hand-written variants and cost nothing to
//! read. Targets are returned as written and are not validated as slugs, so `doctor` can
//! say "this link can never resolve" rather than this module silently dropping it. The
//! one change is composition to NFC, so `[[für]]` typed either way names the same note.

use crate::blocks::Fence;

/// One occurrence of a link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    /// The note name as written, before any `|alias` or `#heading` suffix.
    pub target: String,
    /// 1-based line in the body.
    pub line: u32,
}

/// Every link occurrence, in document order, including repeats.
pub fn find_links(body: &str) -> Vec<Link> {
    let mut out = Vec::new();
    let mut fence: Option<Fence> = None;
    // The paragraph being accumulated: its text and the line it starts on. Inline code
    // spans may cross line breaks but never blank lines, so we scan per paragraph.
    let mut para = String::new();
    let mut para_line: u32 = 0;

    let mut line_no: u32 = 0;
    for line in body.split_inclusive('\n') {
        line_no += 1;
        let stripped = line.trim_end_matches(['\r', '\n']);

        if let Some(f) = fence {
            if f.closes(stripped) {
                fence = None;
            }
            continue;
        }
        if Fence::opener(stripped).is_some() {
            scan_paragraph(&para, para_line, &mut out);
            para.clear();
            fence = Some(Fence::opener(stripped).unwrap());
            continue;
        }
        if stripped.trim().is_empty() {
            scan_paragraph(&para, para_line, &mut out);
            para.clear();
            continue;
        }
        if para.is_empty() {
            para_line = line_no;
        }
        para.push_str(line);
    }
    scan_paragraph(&para, para_line, &mut out);
    out
}

/// Unique link targets in order of first appearance. This is what `scan` writes back
/// into the frontmatter `links:` field.
pub fn link_targets(body: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    find_links(body)
        .into_iter()
        .filter_map(|l| seen.insert(l.target.clone()).then_some(l.target))
        .collect()
}

/// Scan one paragraph for `[[...]]` outside inline code spans.
fn scan_paragraph(text: &str, first_line: u32, out: &mut Vec<Link>) {
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            // A backtick string of length n opens a code span that ends at the next
            // backtick string of exactly length n. Without a closer it is literal text.
            let n = backtick_run(bytes, i);
            match find_closer(bytes, i + n, n) {
                Some(j) => i = j + n,
                None => i += n,
            }
            continue;
        }
        if bytes[i..].starts_with(b"[[") {
            if let Some(rel) = text[i + 2..].find("]]") {
                let inner = &text[i + 2..i + 2 + rel];
                if !inner.contains(['[', ']', '\n', '\r']) {
                    let target = inner.split(['|', '#']).next().unwrap_or("").trim();
                    if !target.is_empty() {
                        let line = first_line + text[..i].matches('\n').count() as u32;
                        out.push(Link {
                            target: crate::frontmatter::normalize_name(target).into_owned(),
                            line,
                        });
                    }
                    i += 2 + rel + 2;
                    continue;
                }
            }
            // `[[` with no usable closer: step past one bracket and keep looking, so
            // `[[[x]]]` still finds `[[x]]`.
            i += 1;
            continue;
        }
        i += 1;
    }
}

fn backtick_run(bytes: &[u8], at: usize) -> usize {
    bytes[at..].iter().take_while(|b| **b == b'`').count()
}

/// Position of the next backtick run of exactly `n`, at or after `from`.
fn find_closer(bytes: &[u8], mut from: usize, n: usize) -> Option<usize> {
    while from < bytes.len() {
        if bytes[from] == b'`' {
            let run = backtick_run(bytes, from);
            if run == n {
                return Some(from);
            }
            from += run;
        } else {
            from += 1;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn targets(body: &str) -> Vec<String> {
        find_links(body).into_iter().map(|l| l.target).collect()
    }

    #[test]
    fn finds_plain_links_with_line_numbers() {
        let body = "see [[alpha]] and [[beta]].\n\nthen [[gamma]]\n";
        assert_eq!(
            find_links(body),
            [
                Link {
                    target: "alpha".into(),
                    line: 1
                },
                Link {
                    target: "beta".into(),
                    line: 1
                },
                Link {
                    target: "gamma".into(),
                    line: 3
                },
            ]
        );
    }

    #[test]
    fn a_decomposed_target_names_the_same_note_as_a_composed_one() {
        assert_eq!(
            targets("[[fu\u{308}r-kunden]] and [[für-kunden]]"),
            ["für-kunden", "für-kunden"]
        );
    }

    #[test]
    fn nonexistent_targets_are_returned_not_rejected() {
        // This module cannot know what exists; a dangling link is intent (SPEC §3.1).
        assert_eq!(targets("[[does-not-exist-yet]]"), ["does-not-exist-yet"]);
    }

    #[test]
    fn dedups_in_first_seen_order() {
        let body = "[[b]] [[a]] [[b]]\n[[c]] [[a]]";
        assert_eq!(link_targets(body), ["b", "a", "c"]);
        assert_eq!(find_links(body).len(), 5);
    }

    #[test]
    fn alias_and_heading_suffixes_are_stripped() {
        assert_eq!(
            targets("[[note|shown text]] [[other#section]] [[ padded ]]"),
            ["note", "other", "padded"]
        );
    }

    #[test]
    fn ignores_links_inside_code_fences() {
        let body = "[[one]]\n\n```\n[[two]]\n```\n\n[[three]]\n~~~md\n[[four]]\n~~~\n[[five]]";
        assert_eq!(targets(body), ["one", "three", "five"]);
    }

    #[test]
    fn unclosed_fence_hides_everything_after_it() {
        assert_eq!(targets("[[a]]\n```\n[[b]]\n[[c]]"), ["a"]);
    }

    #[test]
    fn ignores_links_inside_inline_code() {
        assert_eq!(targets("use `[[not-a-link]]` but [[real]]"), ["real"]);
        assert_eq!(targets("``a ` [[nope]] `` [[yes]]"), ["yes"]);
        // A lone backtick without a closer is literal text.
        assert_eq!(targets("a ` b [[yes]]"), ["yes"]);
        // Different run lengths do not close each other.
        assert_eq!(targets("`` x ` [[nope]] `` [[yes]]"), ["yes"]);
    }

    #[test]
    fn inline_code_may_span_a_line_break_but_not_a_blank_line() {
        assert_eq!(targets("`a\n[[nope]]` [[yes]]"), ["yes"]);
        assert_eq!(targets("`a\n\n[[yes]]"), ["yes"]);
    }

    #[test]
    fn malformed_brackets_do_not_swallow_the_rest() {
        assert_eq!(targets("[[unclosed then [[ok]]"), ["ok"]);
        assert_eq!(targets("[[[triple]]]"), ["triple"]);
        assert_eq!(targets("[[]] [[ ]] [[|alias-only]] [[x]]"), ["x"]);
        assert_eq!(targets("[[multi\nline]] [[x]]"), ["x"]);
        assert_eq!(targets("[[a]b]] [[x]]"), ["x"]);
    }

    #[test]
    fn empty_body_has_no_links() {
        assert!(find_links("").is_empty());
        assert!(link_targets("no links here").is_empty());
    }
}
