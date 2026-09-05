//! Go.
//!
//! Definitions: `func` (a method when it has a receiver; the receiver's type is the
//! scope), `type` (struct, interface, or alias/named type), `const` and `var` at package
//! level, including the names inside `const (` / `var (` blocks.
//!
//! Left out: interface method signatures, struct fields, `:=` locals. The `//` comment
//! block directly above a declaration is included in its range, because that is where Go
//! puts documentation.

use super::{Def, caps};
use crate::DefKind;
use crate::extent::{Lexed, Style, Terminator, extend_up, item_extent};
use regex::Regex;
use std::sync::LazyLock;

static FUNC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^func\s+([A-Za-z_]\w*)").unwrap());
static METHOD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^func\s*\(\s*(?:[A-Za-z_]\w*\s+)?\*?([A-Za-z_]\w*)(?:\[[^\]]*\])?\s*\)\s*([A-Za-z_]\w*)",
    )
    .unwrap()
});
static TYPE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^type\s+([A-Za-z_]\w*)(?:\[[^\]]*\])?\s*(=\s*)?(\S+)?").unwrap());
static CONSTVAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(const|var)\s+([A-Za-z_]\w*)").unwrap());
static BLOCK_OPEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(const|var)\s*\(\s*$").unwrap());
static BLOCK_ITEM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*([A-Za-z_]\w*)\b").unwrap());

fn is_item_start(l: &str) -> bool {
    FUNC.is_match(l)
        || METHOD.is_match(l)
        || TYPE.is_match(l)
        || CONSTVAR.is_match(l)
        || BLOCK_OPEN.is_match(l)
}

fn is_doc(l: &str) -> bool {
    l.trim_start().starts_with("//")
}

pub(crate) fn extract(lines: &[&str], out: &mut Vec<Def>) {
    let term = Terminator::SemicolonOrNewline;
    let lx = Lexed::new(lines, Style::CLike);
    let mut block: Option<DefKind> = None;

    for (i, &line) in lines.iter().enumerate() {
        if let Some(kind) = block {
            if line.trim_start().starts_with(')') {
                block = None;
                continue;
            }
            if let Some(c) = caps(&BLOCK_ITEM, line)
                && !line.trim_start().starts_with("//")
            {
                let end = item_extent(lines, &lx, i, term, &is_item_start);
                out.push(Def {
                    kind,
                    name: c[1].to_string(),
                    scope: None,
                    line: i,
                    start: extend_up(lines, i, is_doc),
                    end,
                });
            }
            continue;
        }
        if let Some(c) = caps(&BLOCK_OPEN, line) {
            block = Some(if &c[1] == "const" {
                DefKind::Const
            } else {
                DefKind::Variable
            });
            continue;
        }
        let (kind, name, scope) = if let Some(c) = caps(&METHOD, line) {
            (DefKind::Method, c[2].to_string(), Some(c[1].to_string()))
        } else if let Some(c) = caps(&FUNC, line) {
            (DefKind::Function, c[1].to_string(), None)
        } else if let Some(c) = caps(&TYPE, line) {
            let kind = match c.get(3).map(|m| m.as_str()) {
                Some(s) if s.starts_with("struct") => DefKind::Struct,
                Some(s) if s.starts_with("interface") => DefKind::Interface,
                _ => DefKind::TypeAlias,
            };
            (kind, c[1].to_string(), None)
        } else if let Some(c) = caps(&CONSTVAR, line) {
            (
                if &c[1] == "const" {
                    DefKind::Const
                } else {
                    DefKind::Variable
                },
                c[2].to_string(),
                None,
            )
        } else {
            continue;
        };
        out.push(Def {
            kind,
            name,
            scope,
            line: i,
            start: extend_up(lines, i, is_doc),
            end: item_extent(lines, &lx, i, term, &is_item_start),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::super::defs;
    use crate::{DefKind, Language};

    #[test]
    fn go_definitions() {
        let src = r#"package main

// Server serves.
type Server struct {
	addr string
}

type Handler interface {
	Serve()
}

type ID = int64

const Version = "1"

var (
	// ErrNope is nope.
	ErrNope = errors.New("nope")
	count   int
)

// Start starts.
func (s *Server) Start() error {
	s.helper()
	return nil
}

func helper(x int) int {
	return x
}
"#;
        let d = defs(Language::Go, src);
        let get = |name: &str| {
            d.iter()
                .find(|x| x.1 == name)
                .cloned()
                .unwrap_or_else(|| panic!("{name} missing in {d:?}"))
        };
        assert_eq!(
            get("Server"),
            (DefKind::Struct, "Server".into(), None, 3, 4, 6)
        );
        assert_eq!(get("Handler").0, DefKind::Interface);
        assert_eq!(
            get("ID"),
            (DefKind::TypeAlias, "ID".into(), None, 12, 12, 12)
        );
        assert_eq!(get("Version").0, DefKind::Const);
        assert_eq!(
            get("ErrNope"),
            (DefKind::Variable, "ErrNope".into(), None, 17, 18, 18)
        );
        assert_eq!(get("count").0, DefKind::Variable);
        assert_eq!(
            get("Start"),
            (
                DefKind::Method,
                "Start".into(),
                Some("Server".into()),
                22,
                23,
                26
            )
        );
        assert_eq!(
            get("helper"),
            (DefKind::Function, "helper".into(), None, 28, 28, 30)
        );
        assert!(d.iter().all(|x| x.1 != "Serve" && x.1 != "addr"));
    }
}
