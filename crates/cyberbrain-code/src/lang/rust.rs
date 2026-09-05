//! Rust.
//!
//! Definitions: `fn` (a method when inside an `impl` or `trait`), `struct`, `enum`,
//! `union`, `trait`, `type`, `const`, `static`, `mod`, `macro_rules!`, and `impl` blocks
//! (named after the type they implement, so `find App` also lists `impl App`).
//!
//! Left out: `let` bindings, `use` items, struct fields, enum variants, associated
//! consts/types other than through their `impl`. Doc comments (`///`) and attributes
//! (`#[...]`) directly above an item are included in its range so a reader gets the
//! signature with its documentation.

use super::{Def, caps};
use crate::DefKind;
use crate::extent::{Lexed, Style, Terminator, extend_up, item_extent};
use regex::Regex;
use std::sync::LazyLock;

static ITEM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(\s*)(?:pub(?:\([^)]*\))?\s+)?(?:(?:default|const|async|unsafe|extern(?:\s+\x22[^\x22]*\x22)?)\s+)*(fn|struct|enum|union|trait|type|const|static|mod)\s+(?:mut\s+)?([A-Za-z_]\w*)",
    )
    .unwrap()
});
static MACRO: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(\s*)(?:pub(?:\([^)]*\))?\s+)?macro_rules!\s+([A-Za-z_]\w*)").unwrap()
});
static IMPL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\s*)(?:unsafe\s+)?impl\b(.*)$").unwrap());

fn is_item_start(l: &str) -> bool {
    ITEM.is_match(l) || MACRO.is_match(l) || IMPL.is_match(l) || is_doc_or_attr(l)
}

fn is_doc_or_attr(l: &str) -> bool {
    let t = l.trim_start();
    t.starts_with("///") || (t.starts_with("#[") && !t.starts_with("#!["))
}

/// Skip a balanced `<...>` at the start of `s`.
fn skip_generics(s: &str) -> &str {
    let b = s.as_bytes();
    if b.first() != Some(&b'<') {
        return s;
    }
    let mut depth = 0usize;
    for (i, &c) in b.iter().enumerate() {
        match c {
            b'<' => depth += 1,
            b'>' => {
                depth -= 1;
                if depth == 0 {
                    return &s[i + 1..];
                }
            }
            _ => {}
        }
    }
    ""
}

/// Find ` for ` at generic depth 0.
fn split_for(s: &str) -> Option<(&str, &str)> {
    let b = s.as_bytes();
    let mut depth = 0usize;
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'<' => depth += 1,
            b'>' => depth = depth.saturating_sub(1),
            b'{' => return None,
            b'f' if depth == 0
                && s[i..].starts_with("for ")
                && (i == 0 || b[i - 1].is_ascii_whitespace() || b[i - 1] == b'>') =>
            {
                return Some((&s[..i], &s[i + 4..]));
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// The last path segment of a type: `foo::Bar<T>` -> `Bar`, `&mut Baz` -> `Baz`.
fn type_name(s: &str) -> Option<String> {
    let s = s
        .trim()
        .trim_start_matches('&')
        .trim_start_matches("mut ")
        .trim_start_matches("dyn ");
    let s = s.trim_start_matches("'static ").trim();
    let ident_end = s
        .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':'))
        .unwrap_or(s.len());
    let path = &s[..ident_end];
    let last = path.rsplit("::").next()?;
    if last.is_empty() || !last.chars().next()?.is_alphabetic() && !last.starts_with('_') {
        return None;
    }
    Some(last.to_string())
}

pub(crate) fn extract(lines: &[&str], out: &mut Vec<Def>) {
    // Enclosing blocks that give a scope: (end line, indent, name, is_impl_or_trait).
    let mut scopes: Vec<(usize, usize, String, bool)> = Vec::new();
    let lx = Lexed::new(lines, Style::Rust);

    for (i, &line) in lines.iter().enumerate() {
        while scopes.last().is_some_and(|s| s.0 < i) {
            scopes.pop();
        }
        let scope = scopes.last();

        if let Some(c) = caps(&IMPL, line) {
            let indent = c[1].len();
            let rest = skip_generics(c[2].trim_start());
            let target = match split_for(rest) {
                Some((_trait, ty)) => type_name(ty),
                None => type_name(rest),
            };
            if let Some(name) = target {
                let end = item_extent(lines, &lx, i, Terminator::Semicolon, &is_item_start);
                let start = extend_up(lines, i, is_doc_or_attr);
                out.push(Def {
                    kind: DefKind::Impl,
                    name: name.clone(),
                    scope: scope.map(|s| s.2.clone()),
                    line: i,
                    start,
                    end,
                });
                scopes.push((end, indent, name, true));
            }
            continue;
        }
        if let Some(c) = caps(&MACRO, line) {
            let end = item_extent(lines, &lx, i, Terminator::Semicolon, &is_item_start);
            out.push(Def {
                kind: DefKind::Macro,
                name: c[2].to_string(),
                scope: scope.map(|s| s.2.clone()),
                line: i,
                start: extend_up(lines, i, is_doc_or_attr),
                end,
            });
            continue;
        }
        let Some(c) = caps(&ITEM, line) else {
            continue;
        };
        let indent = c[1].len();
        let keyword = &c[2];
        let name = c[3].to_string();
        let in_impl = scope.is_some_and(|s| s.3);
        let kind = match keyword {
            "fn" if in_impl => DefKind::Method,
            "fn" => DefKind::Function,
            "struct" => DefKind::Struct,
            "enum" => DefKind::Enum,
            "union" => DefKind::Union,
            "trait" => DefKind::Trait,
            "type" => DefKind::TypeAlias,
            "const" => DefKind::Const,
            "static" => DefKind::Static,
            "mod" => DefKind::Module,
            _ => continue,
        };
        let end = item_extent(lines, &lx, i, Terminator::Semicolon, &is_item_start);
        let start = extend_up(lines, i, is_doc_or_attr);
        out.push(Def {
            kind,
            name: name.clone(),
            scope: scope.map(|s| s.2.clone()),
            line: i,
            start,
            end,
        });
        if matches!(kind, DefKind::Trait | DefKind::Module) && end > i {
            scopes.push((end, indent, name, kind == DefKind::Trait));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::defs;
    use crate::{DefKind, Language};

    #[test]
    fn items_methods_and_impls() {
        let src = r#"
/// Doc
#[derive(Debug)]
pub struct App {
    root: u8,
}

impl App {
    /// Opens.
    pub fn open(x: u8) -> App {
        App { root: x }
    }

    fn helper() {}
}

impl<T: Clone> From<T> for Wrapper<T> {
    fn from(t: T) -> Self { Wrapper(t) }
}

pub(crate) const MAX: usize = 5;
static mut COUNT: u32 = 0;
pub type Result<T> = std::result::Result<T, Error>;
pub enum Kind { A, B }
pub trait Sink {
    fn push(&self);
}
mod inner {
    fn hidden() {}
}
macro_rules! say {
    ($e:expr) => { println!("{}", $e) };
}
"#;
        let d = defs(Language::Rust, src);
        let get = |name: &str| {
            d.iter()
                .find(|x| x.1 == name)
                .cloned()
                .unwrap_or_else(|| panic!("{name} missing in {d:?}"))
        };
        assert_eq!(get("App"), (DefKind::Struct, "App".into(), None, 2, 4, 6));
        let open = get("open");
        assert_eq!(
            (open.0, open.2.as_deref(), open.3, open.4, open.5),
            (DefKind::Method, Some("App"), 9, 10, 12)
        );
        let helper = get("helper");
        assert_eq!(
            (helper.0, helper.2.as_deref()),
            (DefKind::Method, Some("App"))
        );
        let from = get("from");
        assert_eq!(
            (from.0, from.2.as_deref()),
            (DefKind::Method, Some("Wrapper"))
        );
        assert!(d.iter().any(|x| x.0 == DefKind::Impl && x.1 == "Wrapper"));
        assert_eq!(get("MAX").0, DefKind::Const);
        assert_eq!(get("COUNT").0, DefKind::Static);
        assert_eq!(get("Result").0, DefKind::TypeAlias);
        assert_eq!(get("Kind").0, DefKind::Enum);
        assert_eq!(get("Sink").0, DefKind::Trait);
        let push = get("push");
        assert_eq!((push.0, push.2.as_deref()), (DefKind::Method, Some("Sink")));
        let hidden = get("hidden");
        assert_eq!(
            (hidden.0, hidden.2.as_deref()),
            (DefKind::Function, Some("inner"))
        );
        assert_eq!(get("say").0, DefKind::Macro);
        // Scopes are only the items that close after us: `helper` is not inside `open`.
        assert!(
            d.iter().all(|x| x.1 != "root"),
            "fields are not definitions"
        );
    }

    #[test]
    fn multi_line_signature_and_where_clause() {
        let src = "pub fn long<T>(\n    a: T,\n) -> T\nwhere\n    T: Clone,\n{\n    a\n}\nfn after() {}\n";
        let d = defs(Language::Rust, src);
        assert_eq!(d[0], (DefKind::Function, "long".into(), None, 1, 1, 8));
        assert_eq!(d[1], (DefKind::Function, "after".into(), None, 9, 9, 9));
    }

    #[test]
    fn calls_and_lets_are_not_definitions() {
        let src = "fn main() {\n    let x = open(1);\n    App::open(2);\n    helper();\n}\n";
        let d = defs(Language::Rust, src);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].1, "main");
    }
}
