//! Markdown.
//!
//! Definitions: ATX headings (`# ...` to `###### ...`), named by their text, scoped by
//! the nearest heading of a lower level, with a range running to the line before the
//! next heading of the same or a higher level. `#` lines inside fenced code blocks are
//! comments, not headings.
//!
//! Left out: setext headings (`===` / `---` underlines), which are indistinguishable from
//! a table rule or a front-matter fence without parsing the paragraph above.

use super::{Def, caps};
use crate::DefKind;
use crate::extent::is_blank;
use regex::Regex;
use std::sync::LazyLock;

static HEADING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(#{1,6})[ \t]+(.+?)[ \t]*#*[ \t]*$").unwrap());

pub(crate) fn extract(lines: &[&str], out: &mut Vec<Def>) {
    let mut fence: Option<&str> = None;
    // (index into out, level)
    let mut open: Vec<(usize, usize)> = Vec::new();
    let mut headings: Vec<(usize, usize)> = Vec::new(); // (line, level)

    for (i, &line) in lines.iter().enumerate() {
        let t = line.trim_start();
        if let Some(f) = fence {
            if t.starts_with(f) {
                fence = None;
            }
            continue;
        }
        if t.starts_with("```") {
            fence = Some("```");
            continue;
        }
        if t.starts_with("~~~") {
            fence = Some("~~~");
            continue;
        }
        let Some(c) = caps(&HEADING, line) else {
            continue;
        };
        let level = c[1].len();
        let name = c[2].trim().to_string();
        if name.is_empty() {
            continue;
        }
        while open.last().is_some_and(|o| o.1 >= level) {
            open.pop();
        }
        let scope = open.last().map(|o| out[o.0].name.clone());
        out.push(Def {
            kind: DefKind::Heading,
            name,
            scope,
            line: i,
            start: i,
            end: i,
        });
        open.push((out.len() - 1, level));
        headings.push((i, level));
    }

    // Ranges: to the line before the next heading of the same or a higher level.
    for (k, &(line, level)) in headings.iter().enumerate() {
        let next = headings[k + 1..]
            .iter()
            .find(|&&(_, l)| l <= level)
            .map(|&(l, _)| l)
            .unwrap_or(lines.len());
        let mut end = next.saturating_sub(1);
        while end > line && is_blank(lines[end]) {
            end -= 1;
        }
        out[k].end = end.max(line);
    }
}

#[cfg(test)]
mod tests {
    use super::super::defs;
    use crate::{DefKind, Language};

    #[test]
    fn headings_with_nesting_and_fences() {
        let src = "# Title\n\nintro\n\n## 10. Code index\n\ntext\n\n```sh\n# not a heading\n```\n\n### Detail ###\n\nmore\n\n## 11. Next\n\nend\n";
        let d = defs(Language::Markdown, src);
        assert_eq!(d[0], (DefKind::Heading, "Title".into(), None, 1, 1, 19));
        assert_eq!(
            d[1],
            (
                DefKind::Heading,
                "10. Code index".into(),
                Some("Title".into()),
                5,
                5,
                15
            )
        );
        assert_eq!(
            d[2],
            (
                DefKind::Heading,
                "Detail".into(),
                Some("10. Code index".into()),
                13,
                13,
                15
            )
        );
        assert_eq!(
            d[3],
            (
                DefKind::Heading,
                "11. Next".into(),
                Some("Title".into()),
                17,
                17,
                19
            )
        );
        assert_eq!(d.len(), 4);
    }
}
