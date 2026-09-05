//! YAML.
//!
//! Definitions: mapping keys at any depth, scoped by the dotted path of their parent keys
//! (`services.backend`), with a range covering the nested block under them. Keys that
//! begin a list item (`- name: x`) are values of the list, not definitions, and are left
//! out; keys on later lines of the same item are kept because that is how a service or a
//! workflow step is named in practice.
//!
//! Block scalars (`key: |`, `key: >`) are skipped as text. Flow mappings (`{a: 1}`) and
//! anchors/aliases are not parsed.

use super::{Def, caps};
use crate::DefKind;
use crate::extent::{Lexed, Style, indent_block_extent, indent_of, is_blank};
use regex::Regex;
use std::sync::LazyLock;

static KEY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^(\s*)([A-Za-z_][\w./\-]*|"[^"]+"|'[^']+')\s*:(?:\s+(.*))?$"#).unwrap()
});

pub(crate) fn extract(lines: &[&str], out: &mut Vec<Def>) {
    // (indent, name)
    let mut scopes: Vec<(usize, String)> = Vec::new();
    let mut skip_until = 0usize;
    let lx = Lexed::new(lines, Style::Python);

    for (i, &line) in lines.iter().enumerate() {
        if i < skip_until {
            continue;
        }
        let t = line.trim_start();
        if is_blank(line) || t.starts_with('#') {
            continue;
        }
        if t == "---" || t == "..." {
            scopes.clear();
            continue;
        }
        let Some(c) = caps(&KEY, line) else {
            continue;
        };
        let indent = indent_of(&c[1]);
        let name = c[2].trim_matches(['"', '\'']).to_string();
        let value = c.get(3).map(|m| m.as_str().trim()).unwrap_or("");

        while scopes.last().is_some_and(|s| s.0 >= indent) {
            scopes.pop();
        }
        let scope = if scopes.is_empty() {
            None
        } else {
            Some(
                scopes
                    .iter()
                    .map(|s| s.1.as_str())
                    .collect::<Vec<_>>()
                    .join("."),
            )
        };
        let end = indent_block_extent(lines, &lx, i, indent);
        if value.starts_with('|') || value.starts_with('>') {
            skip_until = end + 1;
        }
        out.push(Def {
            kind: DefKind::Key,
            name: name.clone(),
            scope,
            line: i,
            start: i,
            end,
        });
        scopes.push((indent, name));
    }
}

#[cfg(test)]
mod tests {
    use super::super::defs;
    use crate::{DefKind, Language};

    #[test]
    fn nested_keys_lists_and_block_scalars() {
        let src = r#"services:
  backend:
    image: x
    command: |
      run:
      not_a_key: 1
    environment:
      - A=1
  redis:
    image: redis
steps:
  - name: Checkout
    uses: actions/checkout
"#;
        let d = defs(Language::Yaml, src);
        assert_eq!(d[0], (DefKind::Key, "services".into(), None, 1, 1, 10));
        assert_eq!(
            d[1],
            (
                DefKind::Key,
                "backend".into(),
                Some("services".into()),
                2,
                2,
                8
            )
        );
        assert_eq!(
            d[2],
            (
                DefKind::Key,
                "image".into(),
                Some("services.backend".into()),
                3,
                3,
                3
            )
        );
        assert_eq!(
            d[3],
            (
                DefKind::Key,
                "command".into(),
                Some("services.backend".into()),
                4,
                4,
                6
            )
        );
        assert_eq!(
            d[4],
            (
                DefKind::Key,
                "environment".into(),
                Some("services.backend".into()),
                7,
                7,
                8
            )
        );
        assert_eq!(
            d[5],
            (
                DefKind::Key,
                "redis".into(),
                Some("services".into()),
                9,
                9,
                10
            )
        );
        assert_eq!(d[6].1, "image");
        assert_eq!(d[7], (DefKind::Key, "steps".into(), None, 11, 11, 13));
        assert_eq!(
            d[8],
            (
                DefKind::Key,
                "uses".into(),
                Some("steps".into()),
                13,
                13,
                13
            )
        );
        assert!(
            d.iter()
                .all(|x| x.1 != "not_a_key" && x.1 != "run" && x.1 != "name"),
            "{d:?}"
        );
    }
}
