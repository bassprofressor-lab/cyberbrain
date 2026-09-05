//! Where a definition ends.
//!
//! A `find` hit is a line range the caller reads *instead of* the file, so the range has to
//! hold the whole definition and nothing that is not part of it. The extractors here are
//! heuristic (SPEC §10: regex/heuristic by default, tree-sitter deferred), and their one
//! hard promise is that the range always contains the defining line. When a scanner cannot
//! tell where a statement ends, it returns the defining line alone rather than guessing a
//! longer range that may be wrong: a short range costs the caller one more read, a wrong
//! one costs them the wrong slice with no way to know.
//!
//! Two families of extent:
//!
//! - **Bracketed** ([`item_extent`], [`value_extent`]): brace-delimited bodies and
//!   `;`-terminated statements, tracked through a small lexer that skips string literals
//!   and comments so a `}` inside a string does not close a function.
//! - **Indented** ([`indent_extent`], [`indent_block_extent`]): Python and YAML, where the
//!   body is every following line indented deeper than the header.

/// How to lex strings and comments for a given language family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Style {
    /// `//` and `/* */` comments, `"` strings, `'` only as a char literal (`'a'`, `'\n'`),
    /// so lifetimes do not open a string that never closes.
    Rust,
    /// `//` and `/* */`, `"` and `'` strings, multi-line backtick strings (JS templates,
    /// Go raw strings).
    CLike,
    /// `#` comments, `"`/`'` strings, `"""`/`'''` multi-line strings.
    Python,
    /// `--` and `/* */` comments, `'`/`"` strings, `$$`/`$tag$` bodies.
    Sql,
    /// `#` comments, `"`/`'` strings, `"""`/`'''` multi-line strings.
    Toml,
    /// `"` strings only, no comments.
    Json,
}

/// Lexer state that survives across lines: the multi-line constructs.
#[derive(Debug, Default, Clone)]
pub(crate) struct LexState {
    block_comment: bool,
    backtick: bool,
    /// `"""` or `'''` opener currently open (Python, TOML).
    triple: Option<char>,
    /// The `$tag$` currently open (SQL); `$$` is the empty tag.
    dollar: Option<String>,
}

impl LexState {
    /// True while a multi-line string or comment is open at a line boundary.
    pub(crate) fn is_open(&self) -> bool {
        self.block_comment || self.backtick || self.triple.is_some() || self.dollar.is_some()
    }
}

/// Append the *code* characters of `line` to `out`: everything outside comments and
/// string literals. Non-ASCII characters are passed through as `b'?'` because only ASCII
/// punctuation is ever inspected. Multi-line state is carried in `st`.
pub(crate) fn code_chars(line: &str, st: &mut LexState, style: Style, out: &mut Vec<u8>) {
    let b = line.as_bytes();
    let n = b.len();
    let mut i = 0;
    while i < n {
        let c = b[i];

        // ----- inside a multi-line construct -------------------------------------------
        if st.block_comment {
            match find_from(b, i, b"*/") {
                Some(j) => i = j + 2,
                None => return,
            }
            st.block_comment = false;
            continue;
        }
        if st.backtick {
            match memchr::memchr(b'`', &b[i..]) {
                Some(j) => i += j + 1,
                None => return,
            }
            st.backtick = false;
            continue;
        }
        if let Some(q) = st.triple {
            let closer = [q as u8, q as u8, q as u8];
            match find_from(b, i, &closer) {
                Some(j) => i = j + 3,
                None => return,
            }
            st.triple = None;
            continue;
        }
        if let Some(tag) = &st.dollar {
            let closer = format!("${tag}$");
            match find_from(b, i, closer.as_bytes()) {
                Some(j) => i = j + closer.len(),
                None => return,
            }
            st.dollar = None;
            continue;
        }

        // ----- comments ---------------------------------------------------------------
        match style {
            Style::Rust | Style::CLike => {
                if c == b'/' && i + 1 < n && b[i + 1] == b'/' {
                    return;
                }
                if c == b'/' && i + 1 < n && b[i + 1] == b'*' {
                    st.block_comment = true;
                    i += 2;
                    continue;
                }
            }
            Style::Python | Style::Toml => {
                if c == b'#' {
                    return;
                }
            }
            Style::Sql => {
                if c == b'-' && i + 1 < n && b[i + 1] == b'-' {
                    return;
                }
                if c == b'/' && i + 1 < n && b[i + 1] == b'*' {
                    st.block_comment = true;
                    i += 2;
                    continue;
                }
            }
            Style::Json => {}
        }

        // ----- strings ----------------------------------------------------------------
        if matches!(style, Style::Python | Style::Toml)
            && (c == b'"' || c == b'\'')
            && i + 2 < n
            && b[i + 1] == c
            && b[i + 2] == c
        {
            st.triple = Some(c as char);
            i += 3;
            continue;
        }
        if style == Style::CLike && c == b'`' {
            st.backtick = true;
            i += 1;
            continue;
        }
        if style == Style::Sql && c == b'$' {
            // `$$` or `$tag$`: a tag is [A-Za-z_][A-Za-z0-9_]*.
            let mut j = i + 1;
            while j < n && (b[j].is_ascii_alphanumeric() || b[j] == b'_') {
                j += 1;
            }
            if j < n && b[j] == b'$' && (j == i + 1 || !b[i + 1].is_ascii_digit()) {
                st.dollar = Some(String::from_utf8_lossy(&b[i + 1..j]).into_owned());
                i = j + 1;
                continue;
            }
        }
        if c == b'"' || (c == b'\'' && style != Style::Rust) {
            // A quote that does not close on this line is not treated as a quote: the
            // alternative — swallowing the rest of the line — hides real braces behind an
            // apostrophe in JSX text or a stray quote in a comment we failed to see.
            if let Some(j) = close_quote(b, i + 1, c, style != Style::Sql) {
                i = j + 1;
                continue;
            }
            out.push(c);
            i += 1;
            continue;
        }
        if c == b'\'' && style == Style::Rust {
            // Char literal: 'x' or '\n' or '\u{..}'; anything else is a lifetime.
            if let Some(j) = rust_char_literal_end(b, i) {
                i = j + 1;
                continue;
            }
            out.push(c);
            i += 1;
            continue;
        }

        out.push(if c.is_ascii() { c } else { b'?' });
        i += 1;
    }
}

fn find_from(b: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if from >= b.len() {
        return None;
    }
    memchr::memmem::find(&b[from..], needle).map(|j| from + j)
}

/// Index of the closing quote, honouring backslash escapes when `escapes` is set (SQL
/// doubles quotes instead, which the scan handles naturally: `''` is close-then-open).
fn close_quote(b: &[u8], from: usize, q: u8, escapes: bool) -> Option<usize> {
    let mut i = from;
    while i < b.len() {
        if escapes && b[i] == b'\\' {
            i += 2;
            continue;
        }
        if b[i] == q {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn rust_char_literal_end(b: &[u8], at: usize) -> Option<usize> {
    // at points at the opening quote.
    let n = b.len();
    if at + 2 >= n {
        return None;
    }
    if b[at + 1] == b'\\' {
        // '\n', '\'', '\u{1F600}'
        let end = memchr::memchr(b'\'', &b[at + 2..]).map(|j| at + 2 + j)?;
        return (end - at <= 12).then_some(end);
    }
    // One (possibly multi-byte) character then a quote.
    let ch_len = utf8_len(b[at + 1]);
    let end = at + 1 + ch_len;
    (end < n && b[end] == b'\'').then_some(end)
}

fn utf8_len(first: u8) -> usize {
    if first < 0x80 {
        1
    } else if first >> 5 == 0b110 {
        2
    } else if first >> 4 == 0b1110 {
        3
    } else {
        4
    }
}

/// A file lexed once: the code characters of every line and whether a multi-line
/// construct is still open at the end of each. Extents walk this instead of re-lexing,
/// which matters because a method inside an `impl` inside a `mod` would otherwise be
/// lexed once per enclosing item; measured on this tree the pre-lex halves the scan.
pub(crate) struct Lexed {
    buf: Vec<u8>,
    spans: Vec<(usize, usize)>,
    open_after: Vec<bool>,
}

impl Lexed {
    pub(crate) fn new(lines: &[&str], style: Style) -> Lexed {
        let mut st = LexState::default();
        let mut buf = Vec::with_capacity(lines.iter().map(|l| l.len()).sum());
        let mut spans = Vec::with_capacity(lines.len());
        let mut open_after = Vec::with_capacity(lines.len());
        for line in lines {
            let s = buf.len();
            code_chars(line, &mut st, style, &mut buf);
            spans.push((s, buf.len()));
            open_after.push(st.is_open());
        }
        Lexed {
            buf,
            spans,
            open_after,
        }
    }

    /// Code characters of line `i`.
    pub(crate) fn code(&self, i: usize) -> &[u8] {
        let (s, e) = self.spans[i];
        &self.buf[s..e]
    }

    /// True when a multi-line string or comment is open after line `i` (so line `i + 1`
    /// begins inside it). `open_before(0)` is false.
    pub(crate) fn open_after(&self, i: usize) -> bool {
        self.open_after[i]
    }

    pub(crate) fn open_before(&self, i: usize) -> bool {
        i > 0 && self.open_after[i - 1]
    }

    fn last_code(&self, i: usize) -> Option<u8> {
        self.code(i)
            .iter()
            .rev()
            .find(|c| !c.is_ascii_whitespace())
            .copied()
    }

    fn first_code(&self, i: usize) -> Option<u8> {
        self.code(i)
            .iter()
            .find(|c| !c.is_ascii_whitespace())
            .copied()
    }
}

/// Whether a statement needs a `;` to end, or may end at a line break (Go, JavaScript).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Terminator {
    Semicolon,
    SemicolonOrNewline,
}

/// Longest extent this module will ever return, as a guard against a lexer that lost
/// track inside a pathological file. A body longer than this is not a real function.
const MAX_EXTENT_LINES: usize = 5_000;

/// Extent (0-based inclusive end line) of an item starting at `start`: either a brace-
/// delimited body (`fn f() { ... }`, `class C { ... }`) or a statement ending in `;`.
///
/// Rules, in order, evaluated over code characters only:
/// - `{` at bracket depth 0 opens the body; the line where it closes ends the item.
/// - `;` at depth 0 before any body ends the item.
/// - With [`Terminator::SemicolonOrNewline`], a line that ends at depth 0 with no
///   continuation on either side of the break ends the item (Go, and JavaScript without
///   semicolons).
/// - A later line for which `item_start` is true, reached while no body is open and the
///   brackets are balanced, means the terminator was never found: the item falls back to
///   its defining line rather than swallowing its neighbour.
/// - An unterminated body runs to end of file; an unterminated statement falls back to
///   the defining line alone.
pub(crate) fn item_extent(
    lines: &[&str],
    lx: &Lexed,
    start: usize,
    term: Terminator,
    item_start: &dyn Fn(&str) -> bool,
) -> usize {
    let mut depth: usize = 0;
    let mut body = false;
    let last = lines.len().saturating_sub(1);
    let stop = (start + MAX_EXTENT_LINES).min(last);
    let mut i = start;
    while i <= stop {
        if i > start && !body && depth == 0 && !lx.open_before(i) && item_start(lines[i]) {
            return start;
        }
        for &c in lx.code(i) {
            match c {
                b'(' | b'[' => depth += 1,
                b')' | b']' => depth = depth.saturating_sub(1),
                b'{' => {
                    if depth == 0 {
                        body = true;
                    }
                    depth += 1;
                }
                b'}' => {
                    depth = depth.saturating_sub(1);
                    if body && depth == 0 {
                        return i;
                    }
                }
                b';' if depth == 0 && !body => return i,
                _ => {}
            }
        }
        if !body && depth == 0 && term == Terminator::SemicolonOrNewline && !lx.open_after(i) {
            let next_first = (i < last).then(|| lx.first_code(i + 1)).flatten();
            let ends_here = match lx.last_code(i) {
                None => i > start, // a blank line after the statement began ends it
                Some(c) => !continues_after(c) && !continues_before(next_first),
            };
            if ends_here {
                return i;
            }
        }
        i += 1;
    }
    if body { stop } else { start }
}

fn continues_after(c: u8) -> bool {
    matches!(
        c,
        b'=' | b','
            | b'('
            | b'['
            | b'{'
            | b'+'
            | b'-'
            | b'*'
            | b'/'
            | b'|'
            | b'&'
            | b'?'
            | b':'
            | b'.'
            | b'<'
            | b'>'
            | b'\\'
            | b'!'
            | b'%'
            | b'^'
    )
}

fn continues_before(c: Option<u8>) -> bool {
    matches!(
        c,
        Some(b'|' | b'&' | b'.' | b'?' | b':' | b')' | b']' | b'}' | b'+' | b'-' | b'=')
    )
}

/// Extent of a `key = value` / `"key": value` line: the value's brackets, if it opens any,
/// else the line itself. Multi-line strings count as open brackets.
pub(crate) fn value_extent(lx: &Lexed, start: usize) -> usize {
    let mut depth: usize = 0;
    let last = lx.spans.len().saturating_sub(1);
    let stop = (start + MAX_EXTENT_LINES).min(last);
    let mut i = start;
    while i <= stop {
        for &c in lx.code(i) {
            match c {
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
        if depth == 0 && !lx.open_after(i) {
            return i;
        }
        i += 1;
    }
    start
}

/// Leading whitespace width, tab = 4. Only ever compared within one file.
pub(crate) fn indent_of(line: &str) -> usize {
    let mut w = 0;
    for c in line.chars() {
        match c {
            ' ' => w += 1,
            '\t' => w += 4,
            _ => break,
        }
    }
    w
}

pub(crate) fn is_blank(line: &str) -> bool {
    line.trim().is_empty()
}

/// Python: the header runs until its brackets close (a multi-line `def f(\n a,\n):`),
/// then the body is every following line indented deeper than the header, blank lines
/// included in the middle and trimmed at the end.
pub(crate) fn indent_extent(lines: &[&str], lx: &Lexed, start: usize) -> usize {
    let base = indent_of(lines[start]);
    let last = lines.len().saturating_sub(1);
    let stop = (start + MAX_EXTENT_LINES).min(last);
    let mut depth: usize = 0;
    let mut header_end = start;
    let mut i = start;
    while i <= stop {
        for &c in lx.code(i) {
            match c {
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
        header_end = i;
        if depth == 0 && !lx.open_after(i) {
            break;
        }
        i += 1;
    }
    indent_block_extent(lines, lx, header_end, base)
}

/// The lines after `from` that are indented deeper than `base` (or blank), ending at the
/// last non-blank one. A line inside an open multi-line string (a docstring, a block
/// scalar) is part of the block whatever its indentation.
pub(crate) fn indent_block_extent(lines: &[&str], lx: &Lexed, from: usize, base: usize) -> usize {
    let last = lines.len().saturating_sub(1);
    let stop = (from + MAX_EXTENT_LINES).min(last);
    let mut end = from;
    let mut j = from + 1;
    while j <= stop {
        let line = lines[j];
        if lx.open_before(j) {
            end = j;
            j += 1;
            continue;
        }
        if is_blank(line) {
            j += 1;
            continue;
        }
        if indent_of(line) > base {
            end = j;
            j += 1;
            continue;
        }
        break;
    }
    end
}

/// Walk upward from `line` over contiguous lines accepted by `pred` (decorators, doc
/// comments, attributes) and return the first of them. Stops at the first line the
/// predicate rejects; a blank line rejects.
pub(crate) fn extend_up(lines: &[&str], line: usize, pred: impl Fn(&str) -> bool) -> usize {
    let mut s = line;
    while s > 0 {
        let above = lines[s - 1];
        if is_blank(above) || !pred(above) {
            break;
        }
        s -= 1;
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(line: &str, style: Style) -> String {
        let mut st = LexState::default();
        let mut out = Vec::new();
        code_chars(line, &mut st, style, &mut out);
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn strings_and_comments_are_not_code() {
        assert_eq!(code(r#"let a = "}"; // }"#, Style::Rust), "let a = ; ");
        assert_eq!(code("x = '}' # }", Style::Python), "x =  ");
        assert_eq!(code("SELECT '}' -- }", Style::Sql), "SELECT  ");
        assert_eq!(
            code("fn f<'a>(x: &'a str) {", Style::Rust),
            "fn f<'a>(x: &'a str) {"
        );
        assert_eq!(code("let c = '{';", Style::Rust), "let c = ;");
        assert_eq!(code("let s = `a{b}`;", Style::CLike), "let s = ;");
        // An apostrophe in JSX text does not swallow the closing brace.
        assert_eq!(code("<p>don't</p> }", Style::CLike), "<p>don't</p> }");
    }

    #[test]
    fn multi_line_constructs_carry_state() {
        let mut st = LexState::default();
        let mut out = Vec::new();
        code_chars("/* a {", &mut st, Style::Rust, &mut out);
        assert!(st.block_comment);
        code_chars("} */ ok", &mut st, Style::Rust, &mut out);
        assert!(!st.block_comment);
        assert_eq!(String::from_utf8(out).unwrap(), " ok");

        let mut st = LexState::default();
        let mut out = Vec::new();
        code_chars("s = \"\"\"{", &mut st, Style::Python, &mut out);
        assert!(st.triple.is_some());
        code_chars("}\"\"\" + x", &mut st, Style::Python, &mut out);
        assert_eq!(String::from_utf8(out).unwrap(), "s =  + x");

        let mut st = LexState::default();
        let mut out = Vec::new();
        code_chars("AS $body$ BEGIN;", &mut st, Style::Sql, &mut out);
        assert_eq!(st.dollar.as_deref(), Some("body"));
        code_chars("END; $body$ LANGUAGE x;", &mut st, Style::Sql, &mut out);
        assert_eq!(String::from_utf8(out).unwrap(), "AS  LANGUAGE x;");
    }

    fn never(_: &str) -> bool {
        false
    }

    fn lx(lines: &[&str], style: Style) -> Lexed {
        Lexed::new(lines, style)
    }

    #[test]
    fn item_extent_bodies_and_statements() {
        let src =
            "fn a() {\n  if x { y }\n}\nconst B: u8 = 1;\nstruct C;\nfn d() ->\n  u8\n{\n  1\n}\n";
        let lines: Vec<&str> = src.lines().collect();
        assert_eq!(
            item_extent(
                &lines,
                &lx(&lines, Style::Rust),
                0,
                Terminator::Semicolon,
                &never
            ),
            2
        );
        assert_eq!(
            item_extent(
                &lines,
                &lx(&lines, Style::Rust),
                3,
                Terminator::Semicolon,
                &never
            ),
            3
        );
        assert_eq!(
            item_extent(
                &lines,
                &lx(&lines, Style::Rust),
                4,
                Terminator::Semicolon,
                &never
            ),
            4
        );
        assert_eq!(
            item_extent(
                &lines,
                &lx(&lines, Style::Rust),
                5,
                Terminator::Semicolon,
                &never
            ),
            9
        );
    }

    #[test]
    fn item_extent_without_semicolons() {
        let src = "const a = 5\nconst b = foo(\n  1,\n)\ntype U =\n  | A\n  | B\nconst c = x +\n  y\nlet d\n";
        let lines: Vec<&str> = src.lines().collect();
        let t = Terminator::SemicolonOrNewline;
        let l = lx(&lines, Style::CLike);
        assert_eq!(item_extent(&lines, &l, 0, t, &never), 0);
        assert_eq!(item_extent(&lines, &l, 1, t, &never), 3);
        assert_eq!(item_extent(&lines, &l, 4, t, &never), 6);
        assert_eq!(item_extent(&lines, &l, 7, t, &never), 8);
        assert_eq!(item_extent(&lines, &l, 9, t, &never), 9);
    }

    #[test]
    fn unterminated_statement_falls_back_to_its_own_line() {
        let src = "const A: &str = \"open\nfn later() {}\n";
        let lines: Vec<&str> = src.lines().collect();
        let starts_item = |l: &str| l.starts_with("fn ");
        assert_eq!(
            item_extent(
                &lines,
                &lx(&lines, Style::Rust),
                0,
                Terminator::Semicolon,
                &starts_item
            ),
            0
        );
        // Without the guard it would swallow `later`: that is the range this rule exists for.
        assert_eq!(
            item_extent(
                &lines,
                &lx(&lines, Style::Rust),
                0,
                Terminator::Semicolon,
                &never
            ),
            1
        );
    }

    #[test]
    fn indent_extent_python() {
        let src = "def f(\n    a,\n):\n    x = 1\n\n    return x\n\ndef g():\n    pass\n";
        let lines: Vec<&str> = src.lines().collect();
        let l = lx(&lines, Style::Python);
        assert_eq!(indent_extent(&lines, &l, 0), 5);
        assert_eq!(indent_extent(&lines, &l, 7), 8);
    }

    #[test]
    fn indent_extent_keeps_docstrings_whole() {
        let src =
            "def f():\n    \"\"\"Doc\nnot indented but inside\n    \"\"\"\n    return 1\nx = 2\n";
        let lines: Vec<&str> = src.lines().collect();
        assert_eq!(indent_extent(&lines, &lx(&lines, Style::Python), 0), 4);
    }

    #[test]
    fn value_extent_brackets() {
        let src = "a = [\n  1,\n]\nb = 2\nc = \"\"\"\nx\n\"\"\"\n";
        let lines: Vec<&str> = src.lines().collect();
        let l = lx(&lines, Style::Toml);
        assert_eq!(value_extent(&l, 0), 2);
        assert_eq!(value_extent(&l, 3), 3);
        assert_eq!(value_extent(&l, 4), 6);
    }

    #[test]
    fn extend_up_is_contiguous() {
        let lines = ["/// doc", "", "#[test]", "/// more", "fn f() {}"];
        assert_eq!(
            extend_up(&lines, 4, |l| l.starts_with("///") || l.starts_with("#[")),
            2
        );
        assert_eq!(extend_up(&lines, 0, |_| true), 0);
    }
}
