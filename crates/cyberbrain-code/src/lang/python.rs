//! Python.
//!
//! Definitions: `def` / `async def` (a method when directly inside a `class`), `class`,
//! and module-level assignments at column 0 (`ENGINES = [...]` is a constant when the
//! name is UPPER_CASE, otherwise a variable — `router = APIRouter()` is a thing an agent
//! looks for).
//!
//! Left out: assignments inside functions and classes (attributes, locals), `import`
//! names, `global`/`nonlocal`. Decorators directly above a definition are included in
//! its range: `@router.get("/path")` is half the meaning of the handler under it.

use super::{Def, caps};
use crate::DefKind;
use crate::extent::{Lexed, Style, extend_up, indent_extent, indent_of, value_extent};
use regex::Regex;
use std::sync::LazyLock;

static DEF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\s*)(?:async\s+)?def\s+([A-Za-z_]\w*)").unwrap());
static CLASS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\s*)class\s+([A-Za-z_]\w*)").unwrap());
static ASSIGN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([A-Za-z_]\w*)\s*(?::\s*[^=\n]+?)?\s*=(?:[^=]|$)").unwrap());

fn is_upper(name: &str) -> bool {
    name.chars().any(|c| c.is_ascii_uppercase()) && !name.chars().any(|c| c.is_ascii_lowercase())
}

pub(crate) fn extract(lines: &[&str], out: &mut Vec<Def>) {
    // (indent, name, is_class)
    let mut scopes: Vec<(usize, String, bool)> = Vec::new();
    // Skip the body of a multi-line string or a bracketed expression at module level so
    // `x = 1` inside a docstring is not a variable.
    let mut skip_until = 0usize;
    let lx = Lexed::new(lines, Style::Python);

    for (i, &line) in lines.iter().enumerate() {
        if i < skip_until {
            continue;
        }
        let (indent, name, is_class, is_def) = if let Some(c) = caps(&DEF, line) {
            (c[1].len(), c[2].to_string(), false, true)
        } else if let Some(c) = caps(&CLASS, line) {
            (c[1].len(), c[2].to_string(), true, true)
        } else if let Some(c) = caps(&ASSIGN, line) {
            (0, c[1].to_string(), false, false)
        } else {
            continue;
        };
        let indent = indent_of(&line[..indent]);

        if !is_def {
            let end = value_extent(&lx, i);
            skip_until = end + 1;
            out.push(Def {
                kind: if is_upper(&name) {
                    DefKind::Const
                } else {
                    DefKind::Variable
                },
                name,
                scope: None,
                line: i,
                start: i,
                end,
            });
            continue;
        }

        while scopes.last().is_some_and(|s| s.0 >= indent) {
            scopes.pop();
        }
        let parent = scopes.last();
        let kind = if is_class {
            DefKind::Class
        } else if parent.is_some_and(|p| p.2) {
            DefKind::Method
        } else {
            DefKind::Function
        };
        let end = indent_extent(lines, &lx, i);
        let start = extend_up(lines, i, |l| {
            l.trim_start().starts_with('@') && indent_of(l) == indent
        });
        out.push(Def {
            kind,
            name: name.clone(),
            scope: parent.map(|p| p.1.clone()),
            line: i,
            start,
            end,
        });
        scopes.push((indent, name, is_class));
    }
}

#[cfg(test)]
mod tests {
    use super::super::defs;
    use crate::{DefKind, Language};

    #[test]
    fn defs_classes_methods_and_module_assignments() {
        let src = r#"import os

ENGINES = [
    "carry",
]
router = APIRouter()


class Account:
    """Doc."""

    def __init__(self, x):
        self.x = x

    @property
    async def balance(self) -> int:
        return self.x


@router.get("/ping")
def ping():
    return {"ok": True}


def outer(
    a,
    b,
):
    def inner():
        return a
    return inner
"#;
        let d = defs(Language::Python, src);
        assert_eq!(d[0], (DefKind::Const, "ENGINES".into(), None, 3, 3, 5));
        assert_eq!(d[1], (DefKind::Variable, "router".into(), None, 6, 6, 6));
        assert_eq!(d[2], (DefKind::Class, "Account".into(), None, 9, 9, 17));
        assert_eq!(
            d[3],
            (
                DefKind::Method,
                "__init__".into(),
                Some("Account".into()),
                12,
                12,
                13
            )
        );
        assert_eq!(
            d[4],
            (
                DefKind::Method,
                "balance".into(),
                Some("Account".into()),
                15,
                16,
                17
            )
        );
        assert_eq!(d[5], (DefKind::Function, "ping".into(), None, 20, 21, 22));
        assert_eq!(d[6], (DefKind::Function, "outer".into(), None, 25, 25, 31));
        assert_eq!(
            d[7],
            (
                DefKind::Function,
                "inner".into(),
                Some("outer".into()),
                29,
                29,
                30
            )
        );
        assert_eq!(d.len(), 8, "{d:?}");
    }

    #[test]
    fn calls_and_locals_are_not_definitions() {
        let src = "def f():\n    x = ping()\n    ping()\n    return x\nping()\n";
        let d = defs(Language::Python, src);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].1, "f");
    }
}
