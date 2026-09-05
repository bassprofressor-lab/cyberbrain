//! TOML.
//!
//! Definitions: tables (`[a.b]`, `[[a]]`) as sections named by their last segment with
//! the rest as scope, so `find workspace.dependencies` and `find dependencies` both hit;
//! and keys (`name = ...`) scoped to their table, with a range covering a multi-line
//! value (array, inline table, multi-line string).
//!
//! Left out: keys inside inline tables, dotted keys' intermediate segments.

use super::{Def, caps};
use crate::DefKind;
use crate::extent::{Lexed, Style, is_blank, value_extent};
use regex::Regex;
use std::sync::LazyLock;

static TABLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*\[\[?\s*([^\]]+?)\s*\]\]?").unwrap());
static KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*([A-Za-z0-9_.\-]+|"[^"]*"|'[^']*')\s*="#).unwrap());

fn strip_quotes(s: &str) -> &str {
    s.trim_matches(['"', '\''])
}

pub(crate) fn extract(lines: &[&str], out: &mut Vec<Def>) {
    let mut table: Option<String> = None;
    let mut skip_until = 0usize;
    // Section ranges are closed when the next header appears.
    let mut open_section: Option<usize> = None;
    let lx = Lexed::new(lines, Style::Toml);

    let close_section = |out: &mut Vec<Def>, idx: Option<usize>, before: usize| {
        if let Some(k) = idx {
            let mut end = before.saturating_sub(1);
            while end > out[k].line
                && (is_blank(lines[end]) || lines[end].trim_start().starts_with('#'))
            {
                end -= 1;
            }
            out[k].end = end.max(out[k].line);
        }
    };

    for (i, &line) in lines.iter().enumerate() {
        if i < skip_until {
            continue;
        }
        if let Some(c) = caps(&TABLE, line) {
            close_section(out, open_section, i);
            let full = strip_quotes(c[1].trim()).to_string();
            let (scope, name) = match full.rsplit_once('.') {
                Some((s, n)) => (Some(s.to_string()), n.to_string()),
                None => (None, full.clone()),
            };
            out.push(Def {
                kind: DefKind::Section,
                name,
                scope,
                line: i,
                start: i,
                end: i,
            });
            open_section = Some(out.len() - 1);
            table = Some(full);
            continue;
        }
        if let Some(c) = caps(&KEY, line) {
            let end = value_extent(&lx, i);
            skip_until = end + 1;
            out.push(Def {
                kind: DefKind::Key,
                name: strip_quotes(&c[1]).to_string(),
                scope: table.clone(),
                line: i,
                start: i,
                end,
            });
        }
    }
    close_section(out, open_section, lines.len());
}

#[cfg(test)]
mod tests {
    use super::super::defs;
    use crate::{DefKind, Language};

    #[test]
    fn tables_and_keys() {
        let src = r#"title = "x"

[workspace]
members = [
  "a",
  "b",
]

# comment

[workspace.dependencies]
serde = { version = "1", features = [
  "derive",
] }
"quoted.key" = 1

[[bin]]
name = "cyberbrain"
"#;
        let d = defs(Language::Toml, src);
        assert_eq!(d[0], (DefKind::Key, "title".into(), None, 1, 1, 1));
        assert_eq!(d[1], (DefKind::Section, "workspace".into(), None, 3, 3, 7));
        assert_eq!(
            d[2],
            (
                DefKind::Key,
                "members".into(),
                Some("workspace".into()),
                4,
                4,
                7
            )
        );
        assert_eq!(
            d[3],
            (
                DefKind::Section,
                "dependencies".into(),
                Some("workspace".into()),
                11,
                11,
                15
            )
        );
        assert_eq!(
            d[4],
            (
                DefKind::Key,
                "serde".into(),
                Some("workspace.dependencies".into()),
                12,
                12,
                14
            )
        );
        assert_eq!(
            d[5],
            (
                DefKind::Key,
                "quoted.key".into(),
                Some("workspace.dependencies".into()),
                15,
                15,
                15
            )
        );
        assert_eq!(d[6], (DefKind::Section, "bin".into(), None, 17, 17, 18));
        assert_eq!(
            d[7],
            (DefKind::Key, "name".into(), Some("bin".into()), 18, 18, 18)
        );
    }
}
