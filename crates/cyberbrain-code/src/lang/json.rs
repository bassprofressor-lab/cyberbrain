//! JSON.
//!
//! Definitions: object keys at any depth, one per line, scoped by the dotted path of the
//! keys above them; the range covers the key's value when it is an object or array.
//!
//! Left out: minified JSON (one line, one hit at most — deliberately, a line with a
//! thousand keys is not something an agent reads a slice of), array elements.

use super::{Def, caps};
use crate::DefKind;
use crate::extent::{Lexed, Style, value_extent};
use regex::Regex;
use std::sync::LazyLock;

static KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^\s*"((?:[^"\\]|\\.)*)"\s*:"#).unwrap());

pub(crate) fn extract(lines: &[&str], out: &mut Vec<Def>) {
    // path[d] = the key whose value is the container at depth d+1.
    let mut path: Vec<String> = Vec::new();
    let mut depth: usize = 0;
    let lx = Lexed::new(lines, Style::Json);

    for (i, &line) in lines.iter().enumerate() {
        if let Some(c) = caps(&KEY, line)
            && depth > 0
        {
            let name = c[1].to_string();
            path.truncate(depth - 1);
            let scope = if path.is_empty() {
                None
            } else {
                Some(path.join("."))
            };
            let end = value_extent(&lx, i);
            out.push(Def {
                kind: DefKind::Key,
                name: name.clone(),
                scope,
                line: i,
                start: i,
                end,
            });
            path.push(name);
        }
        for &ch in lx.code(i) {
            match ch {
                b'{' | b'[' => depth += 1,
                b'}' | b']' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::defs;
    use crate::{DefKind, Language};

    #[test]
    fn nested_keys() {
        let src = r#"{
  "name": "ui",
  "scripts": {
    "build": "vite build",
    "dev": "vite"
  },
  "deps": ["a", "b"],
  "nested": { "a": { "b": 1 } },
  "compilerOptions": {
    "paths": {
      "@/*": ["./src/*"]
    }
  }
}
"#;
        let d = defs(Language::Json, src);
        assert_eq!(d[0], (DefKind::Key, "name".into(), None, 2, 2, 2));
        assert_eq!(d[1], (DefKind::Key, "scripts".into(), None, 3, 3, 6));
        assert_eq!(
            d[2],
            (
                DefKind::Key,
                "build".into(),
                Some("scripts".into()),
                4,
                4,
                4
            )
        );
        assert_eq!(d[4], (DefKind::Key, "deps".into(), None, 7, 7, 7));
        assert_eq!(d[5], (DefKind::Key, "nested".into(), None, 8, 8, 8));
        assert_eq!(
            d[6],
            (DefKind::Key, "compilerOptions".into(), None, 9, 9, 13)
        );
        assert_eq!(
            d[7],
            (
                DefKind::Key,
                "paths".into(),
                Some("compilerOptions".into()),
                10,
                10,
                12
            )
        );
        assert_eq!(
            d[8],
            (
                DefKind::Key,
                "@/*".into(),
                Some("compilerOptions.paths".into()),
                11,
                11,
                11
            )
        );
    }
}
