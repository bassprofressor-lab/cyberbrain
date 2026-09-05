//! Cutting one source file into items. The cut is **lossless**: concatenating the item
//! bodies in order gives back the file byte for byte, which is asserted in tests and is
//! the property that makes "nothing was dropped between file and notes" checkable.
//!
//! Headings are CommonMark ATX headings (`#` to `######`, up to three leading spaces, a
//! space after the marks). A heading-looking line inside a fenced code block is text.

use cyberbrain_core::Frontmatter;
use std::path::Path;

/// One item cut from a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// 1-based position in the file.
    pub index: usize,
    /// 1-based line the item starts on.
    pub line: usize,
    /// Heading text without the `#` marks, when the item starts with one.
    pub heading: Option<String>,
    /// The body, verbatim.
    pub body: String,
    /// Text before the first boundary. Only ever the first item, and only when non-blank.
    pub is_preamble: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode<'a> {
    File,
    /// Every heading of `level` or shallower starts an item.
    Heading {
        level: u8,
    },
    /// A line equal to `delimiter` (trimmed) ends an item; the line stays with it.
    Delimiter {
        delimiter: &'a str,
    },
}

/// Split `text`. A file with no boundaries yields one item (a preamble in split modes,
/// so the name template for the preamble applies). Blank text yields nothing.
pub fn split(text: &str, mode: Mode<'_>) -> Vec<Item> {
    if text.trim().is_empty() {
        return Vec::new();
    }
    match mode {
        Mode::File => vec![Item {
            index: 1,
            line: 1,
            heading: first_heading(text),
            body: text.to_string(),
            is_preamble: false,
        }],
        Mode::Heading { level } => {
            let mut items = split_at(text, |line, in_fence| {
                !in_fence && heading_of(line).is_some_and(|(l, _)| l <= level)
            });
            // The file's own title block is the preamble: the first chunk when it has no
            // heading, or a heading shallower than the split level (`# Title` above the
            // `##` sections). A first chunk that starts with a split-level heading is a
            // regular item, not the file's header.
            if let Some(first) = items.first_mut()
                && first.line == 1
            {
                let first_line = first
                    .body
                    .lines()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("");
                first.is_preamble = match heading_of(first_line) {
                    None => true,
                    Some((l, _)) => l < level,
                };
            }
            items
        }
        Mode::Delimiter { delimiter } => {
            let d = delimiter.trim();
            split_after(text, |line, in_fence| !in_fence && line.trim() == d)
        }
    }
}

/// Items begin on lines where `starts` is true.
fn split_at(text: &str, starts: impl Fn(&str, bool) -> bool) -> Vec<Item> {
    let mut items: Vec<Item> = Vec::new();
    let mut fence: Option<Fence> = None;
    let mut cur = String::new();
    let mut cur_line = 1usize;
    let mut line_no = 0usize;
    for line in text.split_inclusive('\n') {
        line_no += 1;
        let stripped = line.trim_end_matches(['\r', '\n']);
        let in_fence = fence.is_some();
        if starts(stripped, in_fence) {
            let blank = cur.trim().is_empty();
            if push(&mut items, cur_line, &mut cur, blank) {
                cur_line = line_no;
            }
        }
        cur.push_str(line);
        fence = track_fence(fence, stripped);
    }
    let blank = cur.trim().is_empty();
    push(&mut items, cur_line, &mut cur, blank);
    items
}

/// Items end after lines where `ends` is true.
fn split_after(text: &str, ends: impl Fn(&str, bool) -> bool) -> Vec<Item> {
    let mut items: Vec<Item> = Vec::new();
    let mut fence: Option<Fence> = None;
    let mut cur = String::new();
    let mut cur_line = 1usize;
    let mut line_no = 0usize;
    for line in text.split_inclusive('\n') {
        line_no += 1;
        let stripped = line.trim_end_matches(['\r', '\n']);
        let in_fence = fence.is_some();
        cur.push_str(line);
        fence = track_fence(fence, stripped);
        if ends(stripped, in_fence) {
            // A chunk that is only its own delimiter carries nothing.
            let blank = cur[..cur.len() - line.len()].trim().is_empty();
            if push(&mut items, cur_line, &mut cur, blank) {
                cur_line = line_no + 1;
            }
        }
    }
    let blank = cur.trim().is_empty();
    push(&mut items, cur_line, &mut cur, blank);
    items
}

/// Close the chunk in `cur` as an item. Returns whether `cur` was consumed. A `blank`
/// chunk carries nothing and never becomes a note: it is folded into the previous item,
/// or — before any item exists — left in `cur` so it rides along with the first real one.
/// The concatenation stays lossless either way.
fn push(items: &mut Vec<Item>, line: usize, cur: &mut String, blank: bool) -> bool {
    if cur.is_empty() {
        return true;
    }
    if blank {
        return match items.last_mut() {
            Some(last) => {
                last.body.push_str(cur);
                cur.clear();
                true
            }
            None => false,
        };
    }
    let body = std::mem::take(cur);
    let index = items.len() + 1;
    items.push(Item {
        index,
        line,
        heading: first_heading(&body),
        body,
        is_preamble: false,
    });
    true
}

/// `(level, text)` if the line is an ATX heading.
pub fn heading_of(line: &str) -> Option<(u8, String)> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let hashes = trimmed.bytes().take_while(|b| *b == b'#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &trimmed[hashes..];
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
        return None;
    }
    let text = rest.trim().trim_end_matches('#').trim();
    Some((hashes as u8, text.to_string()))
}

/// The first heading in `text`, outside fences.
fn first_heading(text: &str) -> Option<String> {
    let mut fence: Option<Fence> = None;
    for line in text.lines() {
        if fence.is_none()
            && let Some((_, h)) = heading_of(line)
        {
            return Some(h);
        }
        fence = track_fence(fence, line);
    }
    None
}

/// The first non-blank line, for naming items that have no heading. Leading Markdown
/// markers (`>`, `-`, `*`, `|`, `#`) are stripped.
pub fn first_text_line(text: &str) -> Option<String> {
    let mut fence: Option<Fence> = None;
    for line in text.lines() {
        if fence.is_none() && !line.trim().is_empty() {
            let t = line
                .trim()
                .trim_start_matches(['>', '-', '*', '|', '#', ' '])
                .trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
        fence = track_fence(fence, line);
    }
    None
}

#[derive(Debug, Clone, Copy)]
struct Fence {
    ch: u8,
    len: usize,
}

fn track_fence(cur: Option<Fence>, line: &str) -> Option<Fence> {
    let t = line.trim_start_matches(' ');
    if line.len() - t.len() > 3 {
        return cur;
    }
    let b = t.as_bytes();
    let Some(&c) = b.first() else { return cur };
    if c != b'`' && c != b'~' {
        return cur;
    }
    let run = b.iter().take_while(|x| **x == c).count();
    if run < 3 {
        return cur;
    }
    match cur {
        Some(f) => {
            if f.ch == c && run >= f.len && t[run..].trim().is_empty() {
                None
            } else {
                Some(f)
            }
        }
        None => {
            if c == b'`' && t[run..].contains('`') {
                return None;
            }
            Some(Fence { ch: c, len: run })
        }
    }
}

/// What the head of a file is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Head {
    /// No `---` fence at the top.
    None,
    /// A cyberbrain note head. The body is what follows it.
    Cyberbrain {
        front: Box<Frontmatter>,
        body: String,
    },
    /// A `---` block that is not a cyberbrain head (another tool's fields, or malformed).
    /// It is kept in the body verbatim; `reason` says why it was not reused.
    Foreign { reason: String },
}

pub fn detect_head(path: &Path, text: &str) -> Head {
    let t = text.strip_prefix('\u{feff}').unwrap_or(text);
    let first = t.split_inclusive('\n').next().unwrap_or("");
    if first.trim_end_matches(['\r', '\n']) != "---" {
        return Head::None;
    }
    match cyberbrain_core::frontmatter::parse(path, text) {
        Ok(p) => Head::Cyberbrain {
            front: Box::new(p.front),
            body: p.body.to_string(),
        },
        Err(e) => Head::Foreign {
            reason: match e {
                cyberbrain_core::Error::Frontmatter { reason, .. } => reason,
                other => other.to_string(),
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn joined(items: &[Item]) -> String {
        items.iter().map(|i| i.body.as_str()).collect()
    }

    #[test]
    fn heading_split_is_lossless_and_keeps_the_preamble() {
        let text = "# Title\n\nintro\n\n## A\n\na text\n### A.1\nsub\n## B\nb text\n";
        let items = split(text, Mode::Heading { level: 2 });
        assert_eq!(joined(&items), text);
        let heads: Vec<Option<&str>> = items.iter().map(|i| i.heading.as_deref()).collect();
        assert_eq!(heads, [Some("Title"), Some("A"), Some("B")]);
        assert!(items[0].is_preamble);
        assert!(!items[1].is_preamble);
        assert_eq!(items[1].line, 5);
        assert_eq!(items[1].body, "## A\n\na text\n### A.1\nsub\n");
        let deep = split(text, Mode::Heading { level: 3 });
        assert_eq!(joined(&deep), text);
        assert_eq!(deep.len(), 4);
        assert_eq!(deep[2].heading.as_deref(), Some("A.1"));
    }

    #[test]
    fn heading_inside_a_fence_is_text() {
        let text = "## A\n```md\n## not a heading\n```\n## B\n~~~\n## also not\n~~~\n";
        let items = split(text, Mode::Heading { level: 2 });
        assert_eq!(joined(&items), text);
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn a_first_chunk_at_the_split_level_is_an_item_not_the_preamble() {
        let items = split("## S1\na\n## S2\nb\n", Mode::Heading { level: 2 });
        assert_eq!(items.len(), 2);
        assert!(!items[0].is_preamble);
        // Leading blank lines ride along with the first real item and do not become one.
        let text = "\n\n## S1\na\n";
        let items = split(text, Mode::Heading { level: 2 });
        assert_eq!(items.len(), 1);
        assert_eq!(joined(&items), text);
        assert!(!items[0].is_preamble);
        assert_eq!(items[0].heading.as_deref(), Some("S1"));
    }

    #[test]
    fn a_file_without_boundaries_is_one_preamble_item() {
        let text = "no headings here\n";
        let items = split(text, Mode::Heading { level: 2 });
        assert_eq!(items.len(), 1);
        assert!(items[0].is_preamble);
        assert_eq!(items[0].heading, None);
        assert!(split("  \n\n", Mode::Heading { level: 2 }).is_empty());
        assert!(split("", Mode::File).is_empty());
    }

    #[test]
    fn delimiter_split_keeps_the_delimiter_with_the_item_it_ends() {
        let text = "one\n---\ntwo\n\n---\n\nthree\n";
        let items = split(text, Mode::Delimiter { delimiter: "---" });
        assert_eq!(joined(&items), text);
        let bodies: Vec<&str> = items.iter().map(|i| i.body.as_str()).collect();
        assert_eq!(bodies, ["one\n---\n", "two\n\n---\n", "\nthree\n"]);
        assert_eq!(items[1].line, 3);
        // Two delimiters in a row do not make an empty note.
        let text = "a\n---\n---\nb";
        let items = split(text, Mode::Delimiter { delimiter: "---" });
        assert_eq!(joined(&items), text);
        assert_eq!(items.len(), 2);
    }

    #[test]
    fn crlf_is_preserved() {
        let text = "## A\r\nx\r\n## B\r\ny\r\n";
        let items = split(text, Mode::Heading { level: 2 });
        assert_eq!(joined(&items), text);
        assert_eq!(items.len(), 2);
        assert_eq!(items[1].heading.as_deref(), Some("B"));
    }

    #[test]
    fn heading_recognition() {
        assert_eq!(
            heading_of("## Session: x ##"),
            Some((2, "Session: x".into()))
        );
        assert_eq!(heading_of("   # deep"), Some((1, "deep".into())));
        assert_eq!(heading_of("    # code"), None);
        assert_eq!(heading_of("#hashtag"), None);
        assert_eq!(heading_of("####### seven"), None);
        assert_eq!(heading_of("#"), Some((1, String::new())));
        assert_eq!(
            first_text_line("\n> - quoted *x*\n"),
            Some("quoted *x*".into())
        );
    }

    #[test]
    fn head_detection() {
        let p = Path::new("x.md");
        assert_eq!(detect_head(p, "# no\n"), Head::None);
        match detect_head(p, "---\ntitle: foo\n---\nbody\n") {
            Head::Foreign { reason } => assert!(reason.contains("unknown field"), "{reason}"),
            other => panic!("{other:?}"),
        }
        let cb = "---\nid: 01ARZ3NDEKTSV4RRFFQ69G5FAV\nname: a-b\nring: 2\nkind: lesson\ncreated: 2026-09-05T09:12:03Z\nupdated: 2026-09-05T09:12:03Z\ntags: [t]\n---\n\nbody\n";
        match detect_head(p, cb) {
            Head::Cyberbrain { front, body } => {
                assert_eq!(front.name, "a-b");
                assert_eq!(body, "body\n");
            }
            other => panic!("{other:?}"),
        }
    }
}
