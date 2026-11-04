//! Randomised inputs for the parsers that read text an agent or a hand edit wrote
//! (audit 2026-11-04). No fuzzing crate: a fixed-seed generator, so a failure is the same
//! failure on every machine and needs no corpus to reproduce.
//!
//! What is checked is what the modules promise: a malformed note is an error, never a panic
//! (`frontmatter`); the split is a pure function and every block is a verbatim span of the
//! body (`blocks`); a parsed head renders back to a file that parses to the same head.

use cyberbrain_core::Citation;
use cyberbrain_core::blocks::split;
use cyberbrain_core::frontmatter::{normalize_name, parse, render, validate_name};
use std::path::Path;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Pieces that the parsers treat specially, plus multi-byte text to catch byte slicing.
const PIECES: &[&str] = &[
    "---",
    "\n",
    "\n\n",
    "\r\n",
    " ",
    "   ",
    "\t",
    "#",
    "## ",
    "```",
    "~~~",
    "````",
    "`",
    "-",
    ":",
    ": ",
    "'",
    "\"",
    "[",
    "]",
    "[[",
    "]]",
    "{",
    "}",
    "|",
    ">",
    "*",
    "&a",
    "!!str",
    "id",
    "name",
    "ring",
    "kind",
    "created",
    "updated",
    "tags",
    "0",
    "2",
    "9",
    "-1",
    "x",
    "ü",
    "ß",
    "日本",
    "🦀",
    "\u{feff}",
    "\u{301}",
    "\0",
    "r0-",
    "r2-1c131dc70e71",
    "2026-11-04T00:00:00Z",
    "00000000-0000-0000-0000-000000000000",
    "knowledge",
    "a-b",
    "~",
    "null",
];

fn random_text(r: &mut Rng, max_pieces: usize) -> String {
    let n = r.below(max_pieces);
    (0..n).map(|_| PIECES[r.below(PIECES.len())]).collect()
}

const VALID_HEAD: &str = "---\nid: 01M1RMGXQEBV0YBR6J2GYP7B8Q\nname: probe\nring: 2\n\
                          kind: knowledge\ncreated: 2026-11-04T00:00:00Z\nupdated: \
                          2026-11-04T00:00:00Z\n---\n\n";

#[test]
fn random_note_files_are_errors_or_notes_never_panics() {
    let mut r = Rng(0x00c0_ffee_2026_1104);
    let mut parsed_ok = 0;
    for _ in 0..20_000 {
        let text = if r.below(2) == 0 {
            random_text(&mut r, 40)
        } else {
            // A valid head with a random body, or a random line spliced into the head.
            let mut t = VALID_HEAD.to_string();
            if r.below(3) == 0 {
                let at = 4 + r.below(VALID_HEAD.len() - 8);
                let at = (at..t.len()).find(|i| t.is_char_boundary(*i)).unwrap();
                t.insert_str(at, &random_text(&mut r, 6));
            }
            t.push_str(&random_text(&mut r, 60));
            t
        };
        let Ok(parsed) = parse(Path::new("probe.md"), &text) else {
            continue;
        };
        // A head that parsed renders to a file whose head parses to the same thing.
        let again = render(&parsed.front, parsed.body).expect("a parsed head renders");
        let back = parse(Path::new("probe.md"), &again).expect("a rendered note parses");
        assert_eq!(back.front, parsed.front, "{text:?}");
        parsed_ok += 1;
    }
    // Otherwise the round-trip half above checked nothing.
    assert!(parsed_ok > 1_000, "only {parsed_ok} inputs parsed");
}

#[test]
fn random_bodies_split_into_verbatim_spans_the_same_way_twice() {
    let mut r = Rng(0x5eed_0000_0000_0001);
    for _ in 0..20_000 {
        let body = random_text(&mut r, 120);
        let limit = [0u32, 1, 2, 7, 64, 512][r.below(6)];
        let a = split(&body, limit);
        let b = split(&body, limit);
        assert_eq!(a.blocks.len(), b.blocks.len(), "{body:?}");
        for (x, y) in a.blocks.iter().zip(&b.blocks) {
            assert_eq!(x.text, y.text, "{body:?}");
            assert!(
                body.contains(x.text.as_str()),
                "block {:?} is not a span of {body:?}",
                x.text
            );
        }
    }
}

#[test]
fn random_names_and_citations_never_panic() {
    let mut r = Rng(0x0bad_cafe_0000_0002);
    for _ in 0..20_000 {
        let s = random_text(&mut r, 8);
        let _ = validate_name(&s);
        let n = normalize_name(&s);
        if validate_name(&n).is_ok() {
            // A valid name is a single path component: it can never climb out of its ring.
            assert!(
                !n.contains(['/', '\\', '\0']) && n != "." && n != "..",
                "{n:?}"
            );
        }
        let _ = s.parse::<Citation>();
        let _ = format!("r{}-{}", r.below(10), s).parse::<Citation>();
    }
}
