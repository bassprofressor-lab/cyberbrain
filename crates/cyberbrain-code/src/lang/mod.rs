//! Per-language definition extractors (SPEC §10, the "fast regex/heuristic indexer").
//!
//! Each module exposes `extract(lines, out)` and pushes one [`Def`] per definition it
//! recognises. The rule every module follows: **definitions, not mentions.** A call site,
//! an import or a use of a name is never a hit, because `find` exists so an agent reads
//! the slice that defines a thing, and a result that is mostly call sites is a failure of
//! the feature.
//!
//! What "definition" means per language is spelled out at the top of each module, and so
//! is what is deliberately left out.

pub(crate) mod go;
pub(crate) mod js;
pub(crate) mod json;
pub(crate) mod markdown;
pub(crate) mod python;
pub(crate) mod rust;
pub(crate) mod sql;
pub(crate) mod toml;
pub(crate) mod yaml;

use crate::{DefKind, Language};

/// `captures` only after `is_match`: the match test runs on the lazy DFA and rejects the
/// overwhelming majority of lines in a few nanoseconds, while `captures` alone drops to
/// the slower capturing engine for every line. Measured on this tree it is the
/// difference between a scan felt and a scan not felt.
pub(crate) fn caps<'t>(re: &regex::Regex, line: &'t str) -> Option<regex::Captures<'t>> {
    if re.is_match(line) {
        re.captures(line)
    } else {
        None
    }
}

/// A definition as the extractor sees it: line numbers are 0-based indices into the line
/// table here and become 1-based only at the API boundary, in one place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Def {
    pub kind: DefKind,
    pub name: String,
    /// The enclosing named thing: the `impl` target of a method, the class of a Python
    /// method, the table of a TOML key, the parent key of a YAML/JSON key, the parent
    /// heading of a Markdown heading.
    pub scope: Option<String>,
    /// The line that names the symbol. Always within `start..=end`.
    pub line: usize,
    /// First line of the slice; at or before `line` when doc comments, attributes or
    /// decorators precede the definition.
    pub start: usize,
    /// Last line of the slice, inclusive.
    pub end: usize,
}

/// Run the extractor for `lang` over `text`.
///
/// The SQL DDL patterns run over *every* language, not only `.sql` files: this operator's
/// tables are created from strings inside `db.py`, and a table's definition is where the
/// `CREATE TABLE` is, whatever file extension surrounds it. Embedded DDL is only accepted
/// in upper case so that a comment saying "create table for users" is not a table named
/// `for`.
pub(crate) fn extract(lang: Language, text: &str) -> Vec<Def> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    match lang {
        Language::Rust => rust::extract(&lines, &mut out),
        Language::Python => python::extract(&lines, &mut out),
        Language::JavaScript => js::extract(&lines, false, &mut out),
        Language::TypeScript => js::extract(&lines, true, &mut out),
        Language::Go => go::extract(&lines, &mut out),
        Language::Sql => sql::extract(&lines, true, &mut out),
        Language::Toml => toml::extract(&lines, &mut out),
        Language::Yaml => yaml::extract(&lines, &mut out),
        Language::Json => json::extract(&lines, &mut out),
        Language::Markdown => markdown::extract(&lines, &mut out),
    }
    if !matches!(lang, Language::Sql | Language::Markdown | Language::Json) {
        sql::extract(&lines, false, &mut out);
    }
    out.sort_by_key(|d| (d.line, d.start));
    out.dedup_by(|a, b| a.line == b.line && a.name == b.name && a.kind == b.kind);
    out
}

#[cfg(test)]
pub(crate) fn defs(
    lang: Language,
    text: &str,
) -> Vec<(DefKind, String, Option<String>, usize, usize, usize)> {
    extract(lang, text)
        .into_iter()
        .map(|d| (d.kind, d.name, d.scope, d.start + 1, d.line + 1, d.end + 1))
        .collect()
}
