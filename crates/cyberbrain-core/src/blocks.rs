//! Splitting a note body into blocks (SPEC §3.3).
//!
//! A block is the unit a citation points at. Blocks are cut at heading and paragraph
//! boundaries and hold at most [`MAX_BLOCK_TOKENS`] tokens, as counted by
//! [`approx_tokens`] — which is an *approximation*. The real tokenizer lives in
//! `cyberbrain-embed`; nothing here may be mistaken for its count, hence the name on
//! every field and function that carries it.
//!
//! Rules:
//!
//! - A heading is never a block of its own: it stays attached to the text that follows it,
//!   and consecutive headings stay together with that text.
//! - A new heading always starts a new block. Between headings, paragraphs are packed
//!   greedily into blocks up to the limit, so one section of ordinary length is one block.
//! - A fenced code block (``` or ~~~) is atomic. It is never split, even when it exceeds
//!   the limit; that case is returned in [`Split::oversized`] so the caller can say so.
//! - A paragraph that alone exceeds the limit is cut at line boundaries, then at
//!   whitespace. A single run of non-whitespace longer than the limit is atomic and
//!   reported like a fence.
//! - Block text is a verbatim span of the body, so what a citation resolves to is what
//!   the human wrote, layout included.
//!
//! The split is a pure function of the body text: same input, same blocks, same
//! citations. That is what keeps citations stable across reindexing.

use crate::citation::Citation;
use crate::types::{Block, Note};

/// The spec's cap on a block (SPEC §3.3), in approximate tokens.
pub const MAX_BLOCK_TOKENS: u32 = 512;

/// A cheap, deterministic **approximation** of a tokenizer's token count.
///
/// Counting: each run of ASCII letters and digits counts `ceil(len / 4)`, each other
/// non-whitespace character (punctuation, symbols, non-ASCII letters) counts one,
/// whitespace counts nothing. It errs on the high side for English prose and on the low
/// side for nothing common, which is the safe direction for a size cap. It is not the
/// embedder's count and must never be compared with one.
pub fn approx_tokens(text: &str) -> u32 {
    let mut tokens: u32 = 0;
    let mut run: u32 = 0;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            run += 1;
        } else {
            tokens += run.div_ceil(4);
            run = 0;
            if !ch.is_whitespace() {
                tokens += 1;
            }
        }
    }
    tokens + run.div_ceil(4)
}

/// One block of text, before it is given a citation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextBlock {
    /// Verbatim span of the body, trailing whitespace removed.
    pub text: String,
    /// 1-based line in the body where the block starts.
    pub line: u32,
    /// [`approx_tokens`] of `text`. An approximation, not the embedder's count.
    pub approx_tokens: u32,
}

/// Why a block was allowed to exceed the limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OversizedReason {
    /// The block holds a fenced code block, which is never split.
    CodeFence,
    /// A single run of non-whitespace characters longer than the limit.
    UnbreakableRun,
}

/// A block that exceeds the limit, reported rather than silently split or dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Oversized {
    /// Index into [`Split::blocks`].
    pub block_idx: u32,
    pub approx_tokens: u32,
    pub reason: OversizedReason,
}

/// The result of splitting a body.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Split {
    pub blocks: Vec<TextBlock>,
    /// Every block whose `approx_tokens` exceeds the limit, and why.
    pub oversized: Vec<Oversized>,
}

/// Split `body` into blocks of at most `limit` approximate tokens. See the module docs.
pub fn split(body: &str, limit: u32) -> Split {
    let segments = segment(body);
    pack(body, &segments, limit)
}

/// Split a note's body and attach citations, producing the [`Block`]s the index stores.
/// `Block::token_count` carries the approximation; the index may overwrite it with the
/// embedder's real count.
pub fn blocks_of(note: &Note, limit: u32) -> (Vec<Block>, Vec<Oversized>) {
    let split = split(&note.body, limit);
    let blocks = split
        .blocks
        .into_iter()
        .enumerate()
        .map(|(i, b)| {
            let idx = i as u32;
            Block {
                citation: Citation::new(note.front.ring, note.front.id, idx, &b.text),
                note_id: note.front.id,
                idx,
                text: b.text,
                token_count: b.approx_tokens,
            }
        })
        .collect();
    (blocks, split.oversized)
}

// ---------------------------------------------------------------------------------------
// Fences and headings (CommonMark shapes, no external parser).

/// An open code fence: the character used and the run length that closes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Fence {
    ch: u8,
    len: usize,
}

impl Fence {
    /// Recognise a fence opener: up to three spaces, then three or more ``` or ~~~.
    /// A backtick fence's info string may not contain a backtick.
    pub(crate) fn opener(line: &str) -> Option<Fence> {
        let (indent, rest) = strip_indent(line);
        if indent > 3 {
            return None;
        }
        let ch = *rest.as_bytes().first()?;
        if ch != b'`' && ch != b'~' {
            return None;
        }
        let len = rest.bytes().take_while(|b| *b == ch).count();
        if len < 3 {
            return None;
        }
        if ch == b'`' && rest[len..].contains('`') {
            return None;
        }
        Some(Fence { ch, len })
    }

    /// Does `line` close this fence: same character, at least as long, nothing else.
    pub(crate) fn closes(self, line: &str) -> bool {
        let (indent, rest) = strip_indent(line);
        if indent > 3 {
            return false;
        }
        let len = rest.bytes().take_while(|b| *b == self.ch).count();
        len >= self.len && rest[len..].trim().is_empty()
    }
}

fn strip_indent(line: &str) -> (usize, &str) {
    let indent = line.bytes().take_while(|b| *b == b' ').count();
    (indent, &line[indent..])
}

/// ATX heading: up to three spaces, one to six `#`, then a space, tab or end of line.
fn is_heading(line: &str) -> bool {
    let (indent, rest) = strip_indent(line);
    if indent > 3 {
        return false;
    }
    let hashes = rest.bytes().take_while(|b| *b == b'#').count();
    if hashes == 0 || hashes > 6 {
        return false;
    }
    matches!(
        rest.as_bytes().get(hashes),
        None | Some(b' ') | Some(b'\t') | Some(b'\r') | Some(b'\n')
    )
}

// ---------------------------------------------------------------------------------------
// Segmentation: the body as a sequence of headings, paragraphs and fences.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Heading,
    Para,
    Fence,
}

#[derive(Debug, Clone, Copy)]
struct Seg {
    kind: Kind,
    start: usize,
    end: usize,
    tokens: u32,
}

fn seg(body: &str, kind: Kind, start: usize, end: usize) -> Seg {
    Seg {
        kind,
        start,
        end,
        tokens: approx_tokens(&body[start..end]),
    }
}

fn segment(body: &str) -> Vec<Seg> {
    let mut segs = Vec::new();
    let mut para: Option<usize> = None; // start offset of the open paragraph
    let mut fence: Option<(Fence, usize)> = None; // open fence and its start offset
    let mut offset = 0usize;

    for line in body.split_inclusive('\n') {
        let start = offset;
        let end = offset + line.len();
        offset = end;
        let stripped = line.trim_end_matches(['\r', '\n']);

        if let Some((f, fstart)) = fence {
            if f.closes(stripped) {
                segs.push(seg(body, Kind::Fence, fstart, end));
                fence = None;
            }
            continue;
        }
        if let Some(f) = Fence::opener(stripped) {
            if let Some(p) = para.take() {
                segs.push(seg(body, Kind::Para, p, start));
            }
            fence = Some((f, start));
            continue;
        }
        if stripped.trim().is_empty() {
            if let Some(p) = para.take() {
                segs.push(seg(body, Kind::Para, p, start));
            }
            continue;
        }
        if is_heading(stripped) {
            if let Some(p) = para.take() {
                segs.push(seg(body, Kind::Para, p, start));
            }
            segs.push(seg(body, Kind::Heading, start, end));
            continue;
        }
        if para.is_none() {
            para = Some(start);
        }
    }
    if let Some((_, fstart)) = fence {
        // Unclosed fence runs to the end of the document (CommonMark).
        segs.push(seg(body, Kind::Fence, fstart, body.len()));
    }
    if let Some(p) = para {
        segs.push(seg(body, Kind::Para, p, body.len()));
    }
    segs
}

/// Cut an oversized paragraph into pieces: first at line boundaries, then at whitespace.
/// The first piece may hold at most `first_avail` tokens, later pieces `limit`. Every
/// piece holds at least one run of non-whitespace, so a run longer than the limit becomes
/// a piece of its own; `pack` reports it as [`OversizedReason::UnbreakableRun`].
fn split_para(body: &str, s: Seg, first_avail: u32, limit: u32) -> Vec<Seg> {
    // Words with their spans; a "word" is a maximal run of non-whitespace. Line breaks
    // are remembered so we prefer cutting there.
    struct Word {
        start: usize,
        end: usize,
        tokens: u32,
        line_end: bool, // this word is the last on its line
    }
    let text = &body[s.start..s.end];
    let mut words: Vec<Word> = Vec::new();
    let mut pos = 0usize;
    for line in text.split_inclusive('\n') {
        let line_start = pos;
        pos += line.len();
        let mut i = 0usize;
        let bytes = line.as_bytes();
        while i < bytes.len() {
            while i < bytes.len() && line[i..].starts_with(char::is_whitespace) {
                i += line[i..].chars().next().unwrap().len_utf8();
            }
            if i >= bytes.len() {
                break;
            }
            let wstart = i;
            while i < bytes.len() && !line[i..].starts_with(char::is_whitespace) {
                i += line[i..].chars().next().unwrap().len_utf8();
            }
            words.push(Word {
                start: s.start + line_start + wstart,
                end: s.start + line_start + i,
                tokens: approx_tokens(&line[wstart..i]),
                line_end: false,
            });
        }
        if let Some(w) = words.last_mut() {
            w.line_end = true;
        }
    }

    // Greedy: fill a piece up to its budget, preferring to end at a line break if one
    // occurred within the budget.
    let mut pieces = Vec::new();
    let mut i = 0usize;
    let mut budget = first_avail;
    while i < words.len() {
        let mut j = i;
        let mut used = 0u32;
        let mut last_line_end: Option<usize> = None;
        while j < words.len() && (j == i || used + words[j].tokens <= budget) {
            used += words[j].tokens;
            if words[j].line_end {
                last_line_end = Some(j);
            }
            j += 1;
        }
        // Cut back to the last line break unless that would leave nothing, or the
        // whole remainder fits anyway.
        if let Some(le) = last_line_end
            && j < words.len()
            && le + 1 > i
        {
            j = le + 1;
        }
        let start = words[i].start;
        let end = words[j - 1].end;
        pieces.push(seg(body, Kind::Para, start, end));
        i = j;
        budget = limit;
    }
    pieces
}

// ---------------------------------------------------------------------------------------
// Packing segments into blocks.

fn pack(body: &str, segs: &[Seg], limit: u32) -> Split {
    struct Packer<'a> {
        body: &'a str,
        limit: u32,
        cur: Vec<Seg>,
        cur_tokens: u32,
        out: Split,
    }
    impl Packer<'_> {
        fn has_content(&self) -> bool {
            self.cur.iter().any(|s| s.kind != Kind::Heading)
        }
        fn push(&mut self, s: Seg) {
            self.cur_tokens += s.tokens;
            self.cur.push(s);
        }
        fn flush(&mut self) {
            let Some(first) = self.cur.first() else {
                return;
            };
            let last = self.cur.last().unwrap();
            let raw = &self.body[first.start..last.end];
            let text = raw.trim_start_matches(['\r', '\n']).trim_end();
            if text.is_empty() {
                self.cur.clear();
                self.cur_tokens = 0;
                return;
            }
            let approx = approx_tokens(text);
            let line = self.body[..first.start].matches('\n').count() as u32 + 1;
            let idx = self.out.blocks.len() as u32;
            if approx > self.limit {
                let reason = if self.cur.iter().any(|s| s.kind == Kind::Fence) {
                    OversizedReason::CodeFence
                } else {
                    OversizedReason::UnbreakableRun
                };
                self.out.oversized.push(Oversized {
                    block_idx: idx,
                    approx_tokens: approx,
                    reason,
                });
            }
            self.out.blocks.push(TextBlock {
                text: text.to_string(),
                line,
                approx_tokens: approx,
            });
            self.cur.clear();
            self.cur_tokens = 0;
        }
    }

    let mut p = Packer {
        body,
        limit,
        cur: Vec::new(),
        cur_tokens: 0,
        out: Split::default(),
    };

    for &s in segs {
        match s.kind {
            Kind::Heading => {
                if p.has_content() {
                    p.flush();
                }
                p.push(s);
            }
            Kind::Fence => {
                if p.has_content() && p.cur_tokens + s.tokens > limit {
                    p.flush();
                }
                p.push(s);
                if p.cur_tokens > limit {
                    // Atomic and over the limit: it is a block of its own (with its
                    // headings) and flush() reports it.
                    p.flush();
                }
            }
            Kind::Para => {
                if p.has_content() && p.cur_tokens + s.tokens > limit {
                    p.flush();
                }
                let avail = limit.saturating_sub(p.cur_tokens);
                if s.tokens <= avail {
                    p.push(s);
                } else {
                    for (i, piece) in split_para(body, s, avail, limit).into_iter().enumerate() {
                        if i > 0 {
                            p.flush();
                        }
                        p.push(piece);
                    }
                }
            }
        }
    }
    p.flush();
    p.out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Frontmatter, NoteId, NoteKind, PiiState, Ring};
    use std::path::PathBuf;

    fn texts(s: &Split) -> Vec<&str> {
        s.blocks.iter().map(|b| b.text.as_str()).collect()
    }

    #[test]
    fn approximation_is_deterministic_and_roughly_word_sized() {
        assert_eq!(approx_tokens(""), 0);
        assert_eq!(approx_tokens("   \n\t"), 0);
        assert_eq!(approx_tokens("the"), 1);
        assert_eq!(approx_tokens("hello world"), 4);
        assert_eq!(approx_tokens("internationalization"), 5);
        assert_eq!(approx_tokens("a.b"), 3);
        assert_eq!(approx_tokens("日本語"), 3);
        assert_eq!(approx_tokens("x"), approx_tokens("x"));
    }

    #[test]
    fn empty_body_has_no_blocks() {
        assert_eq!(split("", 512), Split::default());
        assert_eq!(split("\n\n  \n", 512), Split::default());
    }

    #[test]
    fn one_short_section_is_one_block() {
        let body = "# Title\n\nFirst paragraph.\n\nSecond paragraph.\n";
        let s = split(body, 512);
        assert_eq!(
            texts(&s),
            ["# Title\n\nFirst paragraph.\n\nSecond paragraph."]
        );
        assert_eq!(s.blocks[0].line, 1);
        assert!(s.oversized.is_empty());
    }

    #[test]
    fn a_heading_starts_a_new_block_and_stays_with_its_text() {
        let body = "intro\n\n## A\n\ntext a\n\n## B\ntext b\n\n### C\n#### D\ntext d\n";
        let s = split(body, 512);
        assert_eq!(
            texts(&s),
            [
                "intro",
                "## A\n\ntext a",
                "## B\ntext b",
                "### C\n#### D\ntext d"
            ]
        );
        assert_eq!(
            s.blocks.iter().map(|b| b.line).collect::<Vec<_>>(),
            [1, 3, 7, 10]
        );
    }

    #[test]
    fn a_trailing_heading_without_text_is_still_a_block() {
        let s = split("text\n\n## Empty section\n", 512);
        assert_eq!(texts(&s), ["text", "## Empty section"]);
    }

    #[test]
    fn hash_without_space_is_not_a_heading() {
        let s = split("#hashtag line\nmore\n", 512);
        assert_eq!(texts(&s), ["#hashtag line\nmore"]);
        let s = split("####### seven\n", 512);
        assert_eq!(texts(&s), ["####### seven"]);
    }

    #[test]
    fn paragraphs_pack_up_to_the_limit_and_split_at_paragraph_boundaries() {
        // 4 tokens each ("word word word word" -> 4).
        let para = "word word word word";
        let body = [para; 5].join("\n\n");
        let s = split(&body, 10);
        assert_eq!(
            texts(&s),
            [
                format!("{para}\n\n{para}"),
                format!("{para}\n\n{para}"),
                para.to_string()
            ]
        );
        assert!(s.oversized.is_empty());
        assert!(s.blocks.iter().all(|b| b.approx_tokens <= 10));
    }

    #[test]
    fn oversized_paragraph_is_cut_at_line_boundaries_first() {
        // Lines are 5, 4 and 6 tokens; a limit of 10 fits the first two together.
        let body = "- one two three\n- four five six\n- seven eight nine\n";
        let s = split(body, 10);
        assert_eq!(
            texts(&s),
            ["- one two three\n- four five six", "- seven eight nine"]
        );
        assert!(s.oversized.is_empty());
    }

    #[test]
    fn oversized_line_is_cut_at_whitespace() {
        let body = "a b c d e f g h";
        let s = split(body, 3);
        assert_eq!(texts(&s), ["a b c", "d e f", "g h"]);
        assert!(s.oversized.is_empty());
    }

    #[test]
    fn heading_keeps_the_first_piece_of_an_oversized_paragraph() {
        let body = "## H\n\na b c d e f";
        let s = split(body, 5);
        // "## H" is 3 tokens (two `#`, one word), leaving room for two words.
        assert_eq!(texts(&s), ["## H\n\na b", "c d e f"]);
    }

    #[test]
    fn unbreakable_run_is_atomic_and_reported() {
        let long = "x".repeat(40); // 10 tokens
        let body = format!("short\n\n{long}\n\nafter");
        let s = split(&body, 4);
        assert_eq!(texts(&s), ["short", long.as_str(), "after"]);
        assert_eq!(
            s.oversized,
            [Oversized {
                block_idx: 1,
                approx_tokens: 10,
                reason: OversizedReason::UnbreakableRun
            }]
        );
    }

    #[test]
    fn code_fence_is_never_split_and_is_reported_when_too_big() {
        let body = "Intro.\n\n```rust\nfn a() {}\n\nfn b() {}\n\n# not a heading\n```\n\nOutro.\n";
        let s = split(body, 512);
        assert_eq!(texts(&s), [body.trim_end()]);
        assert!(s.oversized.is_empty());

        // Now with a limit the fence cannot meet.
        let s = split(body, 6);
        assert_eq!(
            texts(&s),
            [
                "Intro.",
                "```rust\nfn a() {}\n\nfn b() {}\n\n# not a heading\n```",
                "Outro."
            ]
        );
        assert_eq!(s.oversized.len(), 1);
        assert_eq!(s.oversized[0].block_idx, 1);
        assert_eq!(s.oversized[0].reason, OversizedReason::CodeFence);
        assert!(s.oversized[0].approx_tokens > 6);
    }

    #[test]
    fn tilde_fences_and_nested_backticks() {
        let body = "~~~\n```\ninner\n```\n~~~\n\ntail";
        let s = split(body, 512);
        assert_eq!(texts(&s), ["~~~\n```\ninner\n```\n~~~\n\ntail"]);
        let s = split(body, 3);
        assert_eq!(texts(&s), ["~~~\n```\ninner\n```\n~~~", "tail"]);
        assert_eq!(s.oversized[0].reason, OversizedReason::CodeFence);
    }

    #[test]
    fn longer_closing_fence_closes_shorter_opener_but_not_vice_versa() {
        let body = "````\n```\nstill inside\n````\nout";
        let s = split(body, 512);
        assert_eq!(texts(&s), ["````\n```\nstill inside\n````\nout"]);
        let s = split(body, 3);
        assert_eq!(texts(&s), ["````\n```\nstill inside\n````", "out"]);
    }

    #[test]
    fn unclosed_fence_runs_to_the_end() {
        let body = "before\n\n```\nnever closed\n\n# looks like heading\n";
        let s = split(body, 3);
        assert_eq!(
            texts(&s),
            ["before", "```\nnever closed\n\n# looks like heading"]
        );
        assert_eq!(s.oversized[0].reason, OversizedReason::CodeFence);
    }

    #[test]
    fn crlf_bodies_split_the_same_places() {
        let lf = "# T\n\npara one\n\n```\ncode\n```\n\n## U\n\npara two\n";
        let crlf = lf.replace('\n', "\r\n");
        let a = split(lf, 512);
        let b = split(&crlf, 512);
        assert_eq!(a.blocks.len(), b.blocks.len());
        assert_eq!(a.blocks.len(), 2);
        assert_eq!(b.blocks[0].text.replace("\r\n", "\n"), a.blocks[0].text);
    }

    #[test]
    fn split_is_a_pure_function_of_the_body() {
        let body = "# A\n\nsome text\n\n```\ncode\n```\n\n## B\n\nmore ".repeat(20);
        assert_eq!(split(&body, 512), split(&body, 512));
        assert_eq!(split(&body, 30), split(&body, 30));
    }

    #[test]
    fn every_block_is_within_the_limit_unless_reported() {
        let mut body = String::new();
        for i in 0..40 {
            body.push_str(&format!("## Section {i}\n\n"));
            body.push_str(
                &"lorem ipsum dolor sit amet, consectetur adipiscing elit. ".repeat(i % 7 + 1),
            );
            body.push_str("\n\n- item one\n- item two\n\n```\n");
            body.push_str(&"let x = 1;\n".repeat(i % 5));
            body.push_str("```\n\n");
        }
        let limit = 40;
        let s = split(&body, limit);
        let reported: std::collections::HashSet<u32> =
            s.oversized.iter().map(|o| o.block_idx).collect();
        for (i, b) in s.blocks.iter().enumerate() {
            assert!(
                b.approx_tokens <= limit || reported.contains(&(i as u32)),
                "block {i} has {} tokens and is not reported",
                b.approx_tokens
            );
            assert!(!b.text.is_empty());
            assert!(body.contains(&b.text), "block text is a verbatim span");
        }
    }

    #[test]
    fn blocks_of_assigns_sequential_indices_and_stable_citations() {
        let id = NoteId::from_string("01ARZ3NDEKTSV4RRFFQ69G5FAV").unwrap();
        let ts: jiff::Timestamp = "2026-09-05T09:12:03Z".parse().unwrap();
        let note = Note {
            front: Frontmatter {
                id,
                name: "n".into(),
                ring: Ring::Session,
                kind: NoteKind::Session,
                created: ts,
                updated: ts,
                tags: vec![],
                links: vec![],
                bereich: None,
                retention: None,
                pii: PiiState::None,
            },
            body: "# A\n\none\n\n# B\n\ntwo\n".into(),
            path: PathBuf::from("notes/r3/n.md"),
        };
        let (blocks, oversized) = blocks_of(&note, MAX_BLOCK_TOKENS);
        assert!(oversized.is_empty());
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].idx, 0);
        assert_eq!(blocks[1].idx, 1);
        assert!(blocks[0].citation.to_string().starts_with("r3-"));
        assert_eq!(
            blocks[0].citation,
            Citation::new(Ring::Session, id, 0, "# A\n\none")
        );
        assert_eq!(blocks[0].token_count, approx_tokens("# A\n\none"));
        let (again, _) = blocks_of(&note, MAX_BLOCK_TOKENS);
        assert_eq!(again[1].citation, blocks[1].citation);
    }
}
