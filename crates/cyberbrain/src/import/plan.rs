//! The mapping file. Everything source-specific lives here, in data; the code knows
//! Markdown, rings and names and nothing about what wrote the files.
//!
//! Shape (TOML):
//!
//! ```toml
//! version = 1
//! root = "."                  # source tree, relative to the plan file
//! tags = ["imported"]         # added to every note
//!
//! [[skip]]                    # evaluated first; a skipped file is counted, never dropped
//! paths = ["backups/**", "*.json"]
//! reason = "tool state and backups"
//!
//! [[group]]                   # first matching group wins, in file order
//! name = "curated"
//! paths = ["cerebrum.md"]
//! ring = 2
//! kind = "knowledge"
//! split = "heading"           # file | heading | delimiter
//! heading_level = 3           # heading: every heading of level <= 3 starts a note
//! note_name = "{stem}-{heading}"
//! collisions = "report"       # report | hash
//! existing = "keep"           # keep | overwrite
//! tags = ["cerebrum"]
//! ```
//!
//! Path patterns: `*` matches within one path segment, `**` any number of segments, `?`
//! one character. A pattern without `/` is matched against the file name at any depth; a
//! pattern with `/` against the whole path relative to `root`.

use cyberbrain_core::frontmatter::validate_retention;
use cyberbrain_core::{Error, NoteKind, Result, Ring};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// The default mapping, as a plan file. It is data, not code: an operator copies it next
/// to a tree, edits paths and rings, and points `import --plan` at it.
///
/// Meant for a `--print-default-plan` flag (or a `--root` fallback via
/// [`ImportPlan::default_for`]) in the CLI; until that is wired only tests use it.
#[allow(dead_code)]
pub const DEFAULT_PLAN_TOML: &str = include_str!("default-plan.toml");

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportPlan {
    #[serde(default = "one")]
    pub version: u32,
    /// The source tree. Relative paths are resolved against the plan file's directory
    /// by [`load_plan`]; a caller building a plan in code sets it absolute.
    #[serde(default)]
    pub root: Option<PathBuf>,
    /// Tags added to every imported note.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Answer every PII hold with "mark reviewed" instead of holding the item. The CLI's
    /// `--accept-pii` sets this; it is also allowed in the file for unattended runs.
    #[serde(default)]
    pub accept_pii: bool,
    #[serde(default)]
    pub skip: Vec<SkipRule>,
    #[serde(default, rename = "group")]
    pub groups: Vec<Group>,
    /// Where the plan came from, for the report. Not part of the file.
    #[serde(skip)]
    pub origin: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkipRule {
    pub paths: Vec<String>,
    /// Why these are not notes. Printed next to every file the rule catches.
    pub reason: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Group {
    /// Label in the report.
    pub name: String,
    pub paths: Vec<String>,
    pub ring: Ring,
    pub kind: NoteKind,
    #[serde(default)]
    pub split: Split,
    /// `heading` mode: every heading of this level or shallower starts a new item.
    #[serde(default = "two")]
    pub heading_level: u8,
    /// `delimiter` mode: a line equal to this (after trimming) ends an item.
    #[serde(default)]
    pub delimiter: Option<String>,
    /// Name template. Placeholders: `{stem}`, `{dir}`, `{heading}`, `{index}`, `{hash}`.
    /// Default: `{stem}` for `file`, `{stem}-{heading}` otherwise.
    #[serde(default)]
    pub note_name: Option<String>,
    /// Name template for the text before the first item boundary. Default `{stem}`.
    #[serde(default)]
    pub preamble_name: Option<String>,
    #[serde(default)]
    pub collisions: Collisions,
    #[serde(default)]
    pub existing: Existing,
    #[serde(default)]
    pub tags: Vec<String>,
    /// ISO-8601 duration applied to every note of the group.
    #[serde(default)]
    pub retention: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Split {
    #[default]
    File,
    Heading,
    Delimiter,
}

/// What to do when two items of one run derive the same name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Collisions {
    /// Write neither; list both with their sources. The operator changes the template.
    #[default]
    Report,
    /// Append `-{hash}` of the item body to every colliding name. Deterministic and
    /// content-bound, so a re-import finds the same note again; reported per item.
    Hash,
}

/// What to do when the name already exists in the store with different content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Existing {
    /// Leave the store's note alone and report the difference.
    #[default]
    Keep,
    /// Replace it, but only if it was imported from the same source file earlier
    /// (it carries the `src:` tag). A hand-written note is never overwritten.
    Overwrite,
}

fn one() -> u32 {
    1
}
fn two() -> u8 {
    2
}

const PLACEHOLDERS: [&str; 5] = ["{stem}", "{dir}", "{heading}", "{index}", "{hash}"];

impl ImportPlan {
    /// Parse plan text. `origin` names it in errors and the report.
    pub fn parse(text: &str, origin: &str) -> Result<ImportPlan> {
        let mut plan: ImportPlan = toml::from_str(text)
            .map_err(|e| Error::Config(format!("import plan {origin}: {e}")))?;
        plan.origin = origin.to_string();
        plan.validate()?;
        Ok(plan)
    }

    /// The built-in mapping with `root` set to `root`. See [`DEFAULT_PLAN_TOML`].
    #[allow(dead_code)]
    pub fn default_for(root: &Path) -> Result<ImportPlan> {
        let mut plan = Self::parse(DEFAULT_PLAN_TOML, "built-in default plan")?;
        plan.root = Some(root.to_path_buf());
        Ok(plan)
    }

    /// The source tree, or a config error if the plan never said.
    pub fn root(&self) -> Result<&Path> {
        self.root.as_deref().ok_or_else(|| {
            Error::Config(format!(
                "import plan {}: no `root` set; add `root = \"...\"` to the plan",
                self.origin
            ))
        })
    }

    fn validate(&self) -> Result<()> {
        let bad = |what: String| Error::Config(format!("import plan {}: {what}", self.origin));
        if self.version != 1 {
            return Err(bad(format!(
                "version {} is not supported (this reader knows version 1)",
                self.version
            )));
        }
        if self.groups.is_empty() {
            return Err(bad("no [[group]] defined; nothing would be imported".into()));
        }
        for (i, s) in self.skip.iter().enumerate() {
            if s.paths.is_empty() {
                return Err(bad(format!("skip rule #{} has no paths", i + 1)));
            }
            if s.reason.trim().is_empty() {
                return Err(bad(format!(
                    "skip rule #{} has no reason; a skip without a reason is a silent drop with paperwork",
                    i + 1
                )));
            }
        }
        let mut names = std::collections::HashSet::new();
        for g in &self.groups {
            let gbad = |what: String| bad(format!("group `{}`: {what}", g.name));
            if g.name.trim().is_empty() {
                return Err(bad("a group has no name".into()));
            }
            if !names.insert(g.name.as_str()) {
                return Err(bad(format!("group name `{}` is used twice", g.name)));
            }
            if g.paths.is_empty() {
                return Err(gbad("no paths".into()));
            }
            match g.split {
                Split::Heading => {
                    if !(1..=6).contains(&g.heading_level) {
                        return Err(gbad(format!(
                            "heading_level {} is not in 1..=6",
                            g.heading_level
                        )));
                    }
                }
                Split::Delimiter => {
                    if g.delimiter.as_deref().is_none_or(|d| d.trim().is_empty()) {
                        return Err(gbad("split = \"delimiter\" needs a `delimiter`".into()));
                    }
                }
                Split::File => {}
            }
            for t in [&g.note_name, &g.preamble_name].into_iter().flatten() {
                check_template(t).map_err(|why| gbad(format!("template `{t}`: {why}")))?;
            }
            if let Some(r) = &g.retention {
                validate_retention(r).map_err(|why| gbad(format!("retention `{r}`: {why}")))?;
            }
        }
        Ok(())
    }
}

/// A template must consist of literal text and known placeholders, and must contain at
/// least one placeholder (a constant name collides with itself on the second item).
fn check_template(t: &str) -> std::result::Result<(), String> {
    let mut rest = t;
    let mut found = 0;
    while let Some(start) = rest.find('{') {
        let after = &rest[start..];
        let Some(end) = after.find('}') else {
            return Err("unclosed `{`".into());
        };
        let ph = &after[..=end];
        if !PLACEHOLDERS.contains(&ph) {
            return Err(format!(
                "unknown placeholder {ph}; known: {}",
                PLACEHOLDERS.join(", ")
            ));
        }
        found += 1;
        rest = &after[end + 1..];
    }
    if found == 0 {
        return Err("has no placeholder, so every item would get the same name".into());
    }
    Ok(())
}

/// Load a plan file. A relative `root` is resolved against the file's directory; an
/// absent `root` means the directory the plan file sits in.
pub fn load_plan(path: &Path) -> Result<ImportPlan> {
    let text = std::fs::read_to_string(path).map_err(|e| Error::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    let mut plan = ImportPlan::parse(&text, &path.display().to_string())?;
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    plan.root = Some(match plan.root.take() {
        Some(r) if r.is_absolute() => r,
        Some(r) => dir.join(r),
        None => dir,
    });
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_plan_parses_and_validates() {
        let p = ImportPlan::default_for(Path::new("/x")).unwrap();
        assert_eq!(p.root.as_deref(), Some(Path::new("/x")));
        assert!(p.groups.len() >= 5);
        assert!(p.groups.iter().any(|g| g.ring == Ring::Invariant));
        assert!(p.groups.iter().any(|g| g.ring == Ring::Protocol));
        assert!(
            p.groups
                .iter()
                .any(|g| g.paths.iter().any(|x| x.starts_with("PREREG_"))
                    && g.ring == Ring::Knowledge),
            "PREREG files land in ring 2"
        );
    }

    #[test]
    fn rejects_the_shapes_that_would_drop_things_quietly() {
        let base = "version = 1\n[[group]]\nname = \"g\"\npaths = [\"*.md\"]\nring = 2\nkind = \"knowledge\"\n";
        assert!(ImportPlan::parse(base, "t").is_ok());
        for (text, needle) in [
            ("version = 2\n", "version 2"),
            ("version = 1\n", "no [[group]]"),
            (
                "version = 1\n[[skip]]\npaths = [\"x\"]\nreason = \" \"\n[[group]]\nname = \"g\"\npaths = [\"a\"]\nring = 2\nkind = \"bug\"\n",
                "no reason",
            ),
            (
                "version = 1\n[[group]]\nname = \"g\"\npaths = [\"a\"]\nring = 7\nkind = \"bug\"\n",
                "ring",
            ),
            (
                "version = 1\n[[group]]\nname = \"g\"\npaths = [\"a\"]\nring = 2\nkind = \"poem\"\n",
                "poem",
            ),
            (
                "version = 1\n[[group]]\nname = \"g\"\npaths = [\"a\"]\nring = 2\nkind = \"bug\"\nsplit = \"delimiter\"\n",
                "needs a `delimiter`",
            ),
            (
                "version = 1\n[[group]]\nname = \"g\"\npaths = [\"a\"]\nring = 2\nkind = \"bug\"\nnote_name = \"fixed\"\n",
                "no placeholder",
            ),
            (
                "version = 1\n[[group]]\nname = \"g\"\npaths = [\"a\"]\nring = 2\nkind = \"bug\"\nnote_name = \"{nope}\"\n",
                "unknown placeholder",
            ),
            (
                "version = 1\n[[group]]\nname = \"g\"\npaths = [\"a\"]\nring = 2\nkind = \"bug\"\ntypo = 1\n",
                "unknown field",
            ),
            (
                "version = 1\n[[group]]\nname = \"g\"\npaths = [\"a\"]\nring = 2\nkind = \"bug\"\n[[group]]\nname = \"g\"\npaths = [\"b\"]\nring = 2\nkind = \"bug\"\n",
                "used twice",
            ),
        ] {
            let e = ImportPlan::parse(text, "t").unwrap_err().to_string();
            assert!(e.contains(needle), "{text:?} -> {e}");
        }
    }

    #[test]
    fn load_resolves_root_against_the_plan_directory() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("plan.toml");
        std::fs::write(
            &p,
            "version = 1\nroot = \"src\"\n[[group]]\nname = \"g\"\npaths = [\"*.md\"]\nring = 2\nkind = \"knowledge\"\n",
        )
        .unwrap();
        let plan = load_plan(&p).unwrap();
        assert_eq!(plan.root.unwrap(), dir.path().join("src"));
        std::fs::write(
            &p,
            "version = 1\n[[group]]\nname = \"g\"\npaths = [\"*.md\"]\nring = 2\nkind = \"knowledge\"\n",
        )
        .unwrap();
        assert_eq!(load_plan(&p).unwrap().root.unwrap(), dir.path());
    }
}
