//! Cyberbrain code index. See docs/SPEC.md §10.
//!
//! `find(root, symbol, limit, opts)` walks a project tree, extracts *definitions* with
//! per-language heuristics ([`lang`]), matches them against the symbol ([`query`]) and
//! returns line ranges the caller reads instead of whole files.
//!
//! # No persisted index, and why
//!
//! The index is rebuilt on every call. Measured in release mode on this crate's own tree
//! (131 files, 1.4 MiB, 2 688 definitions once `target/` and `node_modules/` are
//! gitignored): 14 ms median, of which 3.7 ms is the walk and reading, about 4.5 ms is
//! regex matching and the rest is lexing for line ranges. A FastAPI backend of 48 files
//! took 4 ms, a Next.js frontend of 97 files 9 ms. A cache keyed on path, size and mtime
//! would still have to `stat` every file to validate itself, so it could only save the
//! 10 ms that are not the walk, and it would add the one failure this feature must not
//! have: a line range that no longer points at the symbol, handed to a caller who then
//! reads the wrong slice without knowing. A fresh scan cannot be stale. If a tree ever
//! grows to where the scan is felt (roughly 100 ms per ten thousand source files), a
//! cache validated by size+mtime is the next step, and the split into walk / extract /
//! match leaves room for it. `find` is not a hook event; a hook that called it would
//! spend its whole 15 ms budget (SPEC §9.1) on this tree, which is a reason for the
//! hook not to call it, not for a cache.
//!
//! # Counts name their boundary (SPEC §14.3)
//!
//! `files_scanned` is files whose text reached an extractor. Every other file is under a
//! reason in [`Skipped`], and a directory the ignore rules kept the walk out of counts
//! once as an entry that was *not entered* — the files inside were never looked at, so
//! no number claims to know how many there were.

mod extent;
mod lang;
mod query;
mod walk;

#[cfg(test)]
mod tests;

use cyberbrain_core::{Error, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The ignore file honoured at every level of the tree (SPEC §10). gitignore syntax.
pub const IGNORE_FILE: &str = ".cyberbrainignore";

/// Files over this size are not read. A source file this big is generated or minified,
/// and a hit inside it is not a slice anybody reads.
pub const DEFAULT_MAX_FILE_BYTES: u64 = 1024 * 1024;

/// Snippet length cap, in characters.
const SNIPPET_CHARS: usize = 160;

#[derive(Debug, Clone)]
pub struct FindOptions {
    pub max_file_bytes: u64,
    /// Honour `.gitignore` files as well as `.cyberbrainignore`. On by default; a tree's
    /// `target/` and `node_modules/` are exactly what the operator meant by them.
    pub honour_gitignore: bool,
    /// Descend into dot-directories and read dot-files. Off by default.
    pub include_hidden: bool,
    /// Directories never entered regardless of rules. The binary passes the store here:
    /// its notes are `recall`'s to search.
    pub exclude: Vec<PathBuf>,
}

impl Default for FindOptions {
    fn default() -> Self {
        Self {
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            honour_gitignore: true,
            include_hidden: false,
            exclude: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Language {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Go,
    Sql,
    Toml,
    Yaml,
    Json,
    Markdown,
}

impl Language {
    pub fn as_str(self) -> &'static str {
        match self {
            Language::Rust => "rust",
            Language::Python => "python",
            Language::JavaScript => "javascript",
            Language::TypeScript => "typescript",
            Language::Go => "go",
            Language::Sql => "sql",
            Language::Toml => "toml",
            Language::Yaml => "yaml",
            Language::Json => "json",
            Language::Markdown => "markdown",
        }
    }

    /// By extension. `None` means no extractor exists for the file and it is counted as
    /// unsupported, never silently dropped.
    pub fn of_path(path: &Path) -> Option<Language> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        Some(match ext.as_str() {
            "rs" => Language::Rust,
            "py" | "pyi" | "pyw" => Language::Python,
            "js" | "mjs" | "cjs" | "jsx" => Language::JavaScript,
            "ts" | "mts" | "cts" | "tsx" => Language::TypeScript,
            "go" => Language::Go,
            "sql" | "psql" | "pgsql" => Language::Sql,
            "toml" => Language::Toml,
            "yaml" | "yml" => Language::Yaml,
            "json" | "jsonc" | "json5" => Language::Json,
            "md" | "markdown" | "mdx" => Language::Markdown,
            _ => return None,
        })
    }
}

/// What kind of definition a hit is. Serialised by [`DefKind::as_str`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DefKind {
    Function,
    Method,
    Class,
    Struct,
    Enum,
    Union,
    Trait,
    Interface,
    TypeAlias,
    Impl,
    Module,
    Namespace,
    Macro,
    Const,
    Static,
    Variable,
    /// SQL `CREATE TABLE`.
    Table,
    /// SQL `CREATE VIEW`.
    View,
    /// SQL `CREATE INDEX`.
    Index,
    /// SQL `CREATE TRIGGER`.
    Trigger,
    /// SQL `CREATE SCHEMA`.
    Schema,
    /// TOML `[table]`.
    Section,
    /// TOML / YAML / JSON key.
    Key,
    /// Markdown heading.
    Heading,
}

impl DefKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DefKind::Function => "function",
            DefKind::Method => "method",
            DefKind::Class => "class",
            DefKind::Struct => "struct",
            DefKind::Enum => "enum",
            DefKind::Union => "union",
            DefKind::Trait => "trait",
            DefKind::Interface => "interface",
            DefKind::TypeAlias => "type",
            DefKind::Impl => "impl",
            DefKind::Module => "module",
            DefKind::Namespace => "namespace",
            DefKind::Macro => "macro",
            DefKind::Const => "const",
            DefKind::Static => "static",
            DefKind::Variable => "variable",
            DefKind::Table => "table",
            DefKind::View => "view",
            DefKind::Index => "index",
            DefKind::Trigger => "trigger",
            DefKind::Schema => "schema",
            DefKind::Section => "section",
            DefKind::Key => "key",
            DefKind::Heading => "heading",
        }
    }
}

/// One definition in the tree. Line numbers are 1-based and `start_line..=end_line` is
/// inclusive; `line` is the line that names the symbol and always lies inside the range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Definition {
    /// Root-relative, forward slashes on every platform.
    pub path: String,
    pub language: Language,
    pub kind: DefKind,
    pub name: String,
    /// The enclosing named thing, when there is one: the `impl` target or class of a
    /// method, the table of a TOML key, the dotted parent path of a YAML/JSON key.
    pub scope: Option<String>,
    pub line: u32,
    pub start_line: u32,
    pub end_line: u32,
    /// The defining line, trimmed, at most [`SNIPPET_CHARS`] characters.
    pub snippet: String,
}

/// How a hit matched the query. Ordered best first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MatchKind {
    Exact,
    CaseInsensitive,
    Contains,
}

impl MatchKind {
    pub fn as_str(self) -> &'static str {
        match self {
            MatchKind::Exact => "exact",
            MatchKind::CaseInsensitive => "case-insensitive",
            MatchKind::Contains => "contains",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub def: Definition,
    pub matched: MatchKind,
}

/// Everything the walk declined, by reason. Every field names the side of the boundary
/// it counts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Skipped {
    /// Entries matched by a `.cyberbrainignore` rule. A directory counts once and was
    /// not entered.
    pub ignored_entries: usize,
    /// Entries matched by a `.gitignore` rule. Same counting.
    pub gitignored_entries: usize,
    /// Dot-files and dot-directories, not entered.
    pub hidden_entries: usize,
    /// Directories in [`FindOptions::exclude`] (the store), not entered.
    pub excluded_entries: usize,
    /// Symbolic links, never followed.
    pub symlinks: usize,
    /// Lockfiles by name (`Cargo.lock`, `package-lock.json`, ...), not read.
    pub lockfiles: usize,
    /// Over [`FindOptions::max_file_bytes`], not read.
    pub too_large: usize,
    /// A NUL byte in the first 8 KiB, not parsed.
    pub binary: usize,
    /// No extractor for the extension, not read.
    pub unsupported: usize,
    /// The unsupported ones by extension, so "why is my `.java` not found" answers itself.
    pub unsupported_by_extension: BTreeMap<String, usize>,
    /// `(root-relative path, error)` for entries the filesystem refused.
    pub unreadable: Vec<(String, String)>,
}

/// The result of one `find`.
#[derive(Debug, Clone)]
pub struct FindResult {
    /// The symbol as given.
    pub symbol: String,
    /// The name part after scope splitting (`find` for `App::find`).
    pub name: String,
    /// The scope part, if the symbol had one and it was used to filter.
    pub scope: Option<String>,
    /// Absolute path of the tree that was scanned.
    pub root: PathBuf,
    /// Hits, best first, at most `limit`.
    pub hits: Vec<Hit>,
    /// Hits before truncation.
    pub matched_total: usize,
    pub truncated: bool,
    pub limit: usize,
    /// Files whose text reached an extractor.
    pub files_scanned: usize,
    pub bytes_scanned: u64,
    /// Definitions extracted across all scanned files, matched or not.
    pub definitions_indexed: usize,
    pub skipped: Skipped,
    /// Root-relative paths of the ignore files that were honoured, in walk order.
    pub ignore_files: Vec<String>,
    /// What the counts cannot say: no ignore file, a symbol defined in several files, a
    /// scope that matched nothing. Never empty out of politeness (SPEC §7, same rule).
    pub caveats: Vec<String>,
    pub elapsed: Duration,
}

/// Every definition in the tree, for callers that want the whole index.
#[derive(Debug, Clone)]
pub struct Scan {
    pub root: PathBuf,
    pub definitions: Vec<Definition>,
    pub files_scanned: usize,
    pub bytes_scanned: u64,
    pub skipped: Skipped,
    pub ignore_files: Vec<String>,
    pub elapsed: Duration,
}

fn snippet_of(line: &str) -> String {
    let t = line.trim();
    if t.chars().count() <= SNIPPET_CHARS {
        return t.to_string();
    }
    let mut s: String = t.chars().take(SNIPPET_CHARS - 1).collect();
    s.push('…');
    s
}

/// Walk `root` and extract every definition. Never stale: reads the files as they are now.
pub fn scan(root: &Path, opts: &FindOptions) -> Result<Scan> {
    let started = Instant::now();
    let mut walk = walk::Walk::new(root, opts)?;
    let mut definitions: Vec<Definition> = Vec::new();
    walk.run(&mut |file| {
        let lines: Vec<&str> = file.text.lines().collect();
        for d in lang::extract(file.language, file.text) {
            debug_assert!(d.start <= d.line && d.line <= d.end && d.end < lines.len().max(1));
            definitions.push(Definition {
                path: file.rel.to_string(),
                language: file.language,
                kind: d.kind,
                name: d.name,
                scope: d.scope,
                line: (d.line + 1) as u32,
                start_line: (d.start + 1) as u32,
                end_line: (d.end + 1) as u32,
                snippet: snippet_of(lines.get(d.line).copied().unwrap_or("")),
            });
        }
    })?;
    Ok(Scan {
        root: walk.root().to_path_buf(),
        definitions,
        files_scanned: walk.files_scanned,
        bytes_scanned: walk.bytes_scanned,
        skipped: walk.skipped,
        ignore_files: walk.ignore_files,
        elapsed: started.elapsed(),
    })
}

/// `cyberbrain find <symbol>` (SPEC §10).
///
/// Errors are user errors (exit code 1): an empty symbol, a zero limit, a root that is
/// not a directory, an ignore file that does not parse. Nothing else fails the call —
/// an unreadable file is a count, not an error.
pub fn find(root: &Path, symbol: &str, limit: usize, opts: &FindOptions) -> Result<FindResult> {
    let q = query::parse(symbol);
    if q.name.is_empty() {
        return Err(Error::Config(
            "find: the symbol is empty; give a name such as `open` or `App::open`".into(),
        ));
    }
    if limit == 0 {
        return Err(Error::Config(
            "find: --limit 0 would return nothing and say nothing; use 1 or more".into(),
        ));
    }
    let scan = scan(root, opts)?;
    let mut caveats = Vec::new();

    let mut hits = query::matches(&scan.definitions, &q, true);
    let mut scope_used = q.scope.clone();
    if hits.is_empty()
        && let Some(s) = &q.scope
    {
        hits = query::matches(&scan.definitions, &q, false);
        if !hits.is_empty() {
            caveats.push(format!(
                "no definition of `{}` inside a scope matching `{s}`; showing every `{}` instead",
                q.name, q.name
            ));
        }
        scope_used = None;
    }
    query::rank(&mut hits);

    let matched_total = hits.len();
    let truncated = matched_total > limit;
    hits.truncate(limit);

    if q.name.chars().count() < query::MIN_CONTAINS_LEN {
        caveats.push(format!(
            "`{}` is shorter than {} characters, so only exact and case-insensitive name matches were considered",
            q.name,
            query::MIN_CONTAINS_LEN
        ));
    }
    let root_ignore = scan.ignore_files.iter().any(|f| f == IGNORE_FILE);
    if !root_ignore {
        caveats.push(format!(
            "no {IGNORE_FILE} at {}; every tree not hidden or gitignored was scanned, so a vendored or archived copy of the project would be listed alongside the live one",
            scan.root.display()
        ));
    }
    let files: std::collections::BTreeSet<&str> = hits
        .iter()
        .filter(|h| h.matched == MatchKind::Exact)
        .map(|h| h.def.path.as_str())
        .collect();
    if files.len() > 1 {
        caveats.push(format!(
            "`{}` is defined in {} files ({}); if one is a copy, add it to {IGNORE_FILE}",
            q.name,
            files.len(),
            files.iter().copied().collect::<Vec<_>>().join(", ")
        ));
    }
    if truncated {
        caveats.push(format!(
            "showing {limit} of {matched_total} matching definitions; raise --limit to see the rest"
        ));
    }
    if scan.skipped.too_large > 0 {
        caveats.push(format!(
            "{} file(s) over {} bytes were not read",
            scan.skipped.too_large, opts.max_file_bytes
        ));
    }

    Ok(FindResult {
        symbol: symbol.to_string(),
        name: q.name,
        scope: scope_used,
        root: scan.root,
        hits,
        matched_total,
        truncated,
        limit,
        files_scanned: scan.files_scanned,
        bytes_scanned: scan.bytes_scanned,
        definitions_indexed: scan.definitions.len(),
        skipped: scan.skipped,
        ignore_files: scan.ignore_files,
        caveats,
        elapsed: scan.elapsed,
    })
}
