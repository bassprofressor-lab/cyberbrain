//! Fixture tests over a real temporary tree with every supported language, plus the
//! walk's accounting. `tempfile` is not a dev-dependency of this crate, so the tree is
//! made by hand under the system temp dir and removed on drop.

use crate::{DefKind, FindOptions, IGNORE_FILE, Language, MatchKind, find, scan};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct TempTree(PathBuf);

impl TempTree {
    fn new() -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "cyberbrain-code-test-{}-{}-{nanos}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&dir).unwrap();
        TempTree(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn write(&self, rel: &str, content: impl AsRef<[u8]>) {
        let p = self.0.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

const RUST: &str = r#"//! A module.

/// Greets.
pub fn greet(name: &str) -> String {
    format!("hi {name}")
}

pub struct Greeter {
    pub prefix: String,
}

impl Greeter {
    pub fn new(prefix: &str) -> Self {
        Greeter { prefix: prefix.into() }
    }

    pub fn greet(&self, name: &str) -> String {
        greet(name)
    }
}

pub const LIMIT: usize = 20;

fn main() {
    let g = Greeter::new("x");
    g.greet("y");
    greet("z");
}
"#;

const PYTHON: &str = r#"from fastapi import APIRouter

router = APIRouter()
ENGINES_WAEHLBAR = ["carry"]


class Greeter:
    def __init__(self, prefix):
        self.prefix = prefix

    def greet(self, name):
        return greet(name)


@router.get("/greet")
def greet(name: str):
    return {"hi": name}


async def init_db(conn):
    await conn.execute("""
        CREATE TABLE IF NOT EXISTS users (
            id serial PRIMARY KEY
        )
    """)
    greet("call site")
"#;

const JS: &str = r#"export const ACTIVE_ENGINES = ["carry"];

export function greet(name) {
  return `hi ${name}`;
}

export default function Dashboard() {
  const handleClick = () => greet("x");
  greet("y");
  return null;
}

class Greeter {
  greet(name) {
    return greet(name);
  }
}
"#;

const TS: &str = r#"export interface Greeter {
  greet(name: string): string;
}
export type Name = string;
export enum Kind { A, B }
export const greet = (name: Name): string => name;
"#;

const GO: &str = r#"package main

type Greeter struct{ prefix string }

func (g *Greeter) Greet(name string) string { return greet(name) }

func greet(name string) string {
	return "hi " + name
}

const Limit = 20
"#;

const SQL: &str = r#"CREATE TABLE greetings (
    id serial PRIMARY KEY,
    text text NOT NULL
);
CREATE INDEX greetings_text ON greetings(text);
"#;

const TOML: &str = r#"[package]
name = "greet"

[dependencies]
greet = { version = "1", features = [
  "a",
] }
"#;

const YAML: &str = r#"services:
  greet:
    image: greet:latest
    ports:
      - "80:80"
  redis:
    image: redis
"#;

const JSON: &str = r#"{
  "name": "greet",
  "scripts": {
    "greet": "node greet.js"
  }
}
"#;

const MD: &str = r#"# Greet

Intro.

## How to greet

Steps.

```sh
# greet is not a heading here
```

## Other
"#;

fn fixture() -> TempTree {
    let t = TempTree::new();
    t.write("src/lib.rs", RUST);
    t.write("backend/app.py", PYTHON);
    t.write("frontend/app.js", JS);
    t.write("frontend/types.ts", TS);
    t.write("svc/main.go", GO);
    t.write("db/schema.sql", SQL);
    t.write("Cargo.toml", TOML);
    t.write("docker-compose.yml", YAML);
    t.write("package.json", JSON);
    t.write("README.md", MD);
    // The vendored copy the spec warns about, and a gitignored build output.
    t.write("vendor/old/src/lib.rs", RUST);
    t.write("build/lib.rs", RUST);
    t.write(IGNORE_FILE, "vendor/\n");
    t.write(".gitignore", "build/\n");
    // Not scanned, each for a named reason.
    t.write(".hidden/lib.rs", RUST);
    t.write("Cargo.lock", "[[package]]\nname = \"greet\"\n");
    t.write("logo.png", b"\x89PNG\r\n\x1a\n");
    t.write("blob.rs", b"fn greet() {}\x00\x00binary");
    t.write("huge.py", "def greet():\n    pass\n".repeat(60_000));
    t
}

fn def_names(root: &Path, symbol: &str) -> Vec<String> {
    find(root, symbol, 100, &FindOptions::default())
        .unwrap()
        .hits
        .into_iter()
        .map(|h| format!("{}:{}:{}", h.def.path, h.def.kind.as_str(), h.def.line))
        .collect()
}

#[test]
fn every_range_contains_the_symbol_on_its_defining_line() {
    let t = fixture();
    let s = scan(t.path(), &FindOptions::default()).unwrap();
    assert!(s.definitions.len() > 30, "{}", s.definitions.len());
    for d in &s.definitions {
        let text = fs::read_to_string(t.path().join(&d.path)).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert!(
            d.start_line >= 1 && d.start_line <= d.line && d.line <= d.end_line,
            "{d:?}"
        );
        assert!(
            (d.end_line as usize) <= lines.len(),
            "{d:?} ends past the file ({} lines)",
            lines.len()
        );
        let defining = lines[(d.line - 1) as usize];
        assert!(
            defining.contains(&d.name),
            "line {} of {} is {defining:?}, which does not contain `{}`",
            d.line,
            d.path,
            d.name
        );
        assert_eq!(d.snippet, defining.trim());
        // The range as a slice also holds the name — the property a reader relies on.
        let slice = lines[(d.start_line - 1) as usize..d.end_line as usize].join("\n");
        assert!(slice.contains(&d.name), "{d:?}");
    }
}

#[test]
fn ignored_copies_do_not_appear_and_are_counted() {
    let t = fixture();
    let r = find(t.path(), "greet", 100, &FindOptions::default()).unwrap();
    let paths: Vec<&str> = r.hits.iter().map(|h| h.def.path.as_str()).collect();
    assert!(
        paths.iter().all(|p| !p.starts_with("vendor/")),
        "the .cyberbrainignore'd copy leaked into the results: {paths:?}"
    );
    assert!(
        paths.iter().all(|p| !p.starts_with("build/")),
        "gitignored copy leaked: {paths:?}"
    );
    assert!(
        paths.iter().all(|p| !p.starts_with(".hidden/")),
        "hidden copy leaked: {paths:?}"
    );
    assert!(
        paths.contains(&"src/lib.rs"),
        "the live copy must be there: {paths:?}"
    );

    assert_eq!(
        r.skipped.ignored_entries, 1,
        "vendor/ counts once, not entered"
    );
    assert_eq!(
        r.skipped.gitignored_entries, 1,
        "build/ counts once, not entered"
    );
    assert_eq!(
        r.skipped.hidden_entries, 3,
        ".hidden/, .cyberbrainignore, .gitignore"
    );
    assert_eq!(r.skipped.lockfiles, 1);
    assert_eq!(r.skipped.binary, 1, "blob.rs holds a NUL");
    assert_eq!(r.skipped.too_large, 1, "huge.py is over the cap");
    assert_eq!(r.skipped.unsupported, 1);
    assert_eq!(r.skipped.unsupported_by_extension.get(".png"), Some(&1));
    assert_eq!(r.skipped.excluded_entries, 0);
    assert!(
        r.skipped.unreadable.is_empty(),
        "{:?}",
        r.skipped.unreadable
    );
    assert_eq!(r.files_scanned, 10);
    assert_eq!(r.ignore_files, vec![".gitignore", IGNORE_FILE]);
    assert!(
        !r.caveats.iter().any(|c| c.contains("no .cyberbrainignore")),
        "{:?}",
        r.caveats
    );
    assert!(
        r.caveats.iter().any(|c| c.contains("were not read")),
        "{:?}",
        r.caveats
    );
}

#[test]
fn without_an_ignore_file_the_copy_is_listed_and_the_caveat_says_so() {
    let t = fixture();
    fs::remove_file(t.path().join(IGNORE_FILE)).unwrap();
    let r = find(t.path(), "greet", 100, &FindOptions::default()).unwrap();
    assert!(r.hits.iter().any(|h| h.def.path.starts_with("vendor/")));
    assert!(
        r.caveats.iter().any(|c| c.contains("no .cyberbrainignore")),
        "{:?}",
        r.caveats
    );
    assert!(
        r.caveats
            .iter()
            .any(|c| c.contains("is defined in") && c.contains("vendor/old/src/lib.rs")),
        "{:?}",
        r.caveats
    );
}

#[test]
fn definitions_not_mentions() {
    let t = fixture();
    let hits = def_names(t.path(), "greet");
    // Rust: the free fn and the method, not the three call sites in main().
    assert!(
        hits.contains(&"src/lib.rs:function:4".to_string()),
        "{hits:?}"
    );
    assert!(
        hits.contains(&"src/lib.rs:method:17".to_string()),
        "{hits:?}"
    );
    assert!(
        !hits.iter().any(|h| h.starts_with("src/lib.rs")
            && (h.ends_with(":25") || h.ends_with(":26") || h.ends_with(":27"))),
        "{hits:?}"
    );
    // Python: the decorated handler and the method, not the call in init_db.
    assert!(
        hits.contains(&"backend/app.py:function:16".to_string()),
        "{hits:?}"
    );
    assert!(
        hits.contains(&"backend/app.py:method:11".to_string()),
        "{hits:?}"
    );
    assert!(
        !hits.contains(&"backend/app.py:function:26".to_string()),
        "{hits:?}"
    );
    // JS: function + class method; not the calls in Dashboard.
    assert!(
        hits.contains(&"frontend/app.js:function:3".to_string()),
        "{hits:?}"
    );
    assert!(
        hits.contains(&"frontend/app.js:method:14".to_string()),
        "{hits:?}"
    );
    // Exactly those two exact matches in app.js; `Greeter` (class, line 13) is a contains-match, nothing else.
    assert_eq!(
        hits.iter()
            .filter(|h| h.starts_with("frontend/app.js"))
            .count(),
        3,
        "{hits:?}"
    );
    assert!(
        hits.contains(&"frontend/app.js:class:13".to_string()),
        "{hits:?}"
    );
    // TS arrow const; Go func; TOML key; YAML key; JSON keys (two: "greet" under scripts only — "name" is the key, not "greet").
    assert!(
        hits.contains(&"frontend/types.ts:function:6".to_string()),
        "{hits:?}"
    );
    assert!(
        hits.contains(&"svc/main.go:function:7".to_string()),
        "{hits:?}"
    );
    assert!(hits.contains(&"Cargo.toml:key:5".to_string()), "{hits:?}");
    assert!(
        hits.contains(&"docker-compose.yml:key:2".to_string()),
        "{hits:?}"
    );
    assert!(hits.contains(&"package.json:key:4".to_string()), "{hits:?}");
    // Markdown: "Greet" and "How to greet" by case-insensitive / contains, never the fenced comment.
    assert!(
        hits.contains(&"README.md:heading:1".to_string()),
        "{hits:?}"
    );
    assert!(
        hits.contains(&"README.md:heading:5".to_string()),
        "{hits:?}"
    );
    assert!(
        !hits.iter().any(|h| h == "README.md:heading:10"),
        "{hits:?}"
    );
}

#[test]
fn ranges_are_exact() {
    let t = fixture();
    let s = scan(t.path(), &FindOptions::default()).unwrap();
    let get = |path: &str, name: &str, kind: DefKind| {
        s.definitions
            .iter()
            .find(|d| d.path == path && d.name == name && d.kind == kind)
            .unwrap_or_else(|| panic!("{path} {name} {kind:?} missing"))
    };
    let d = get("src/lib.rs", "greet", DefKind::Function);
    assert_eq!(
        (d.start_line, d.line, d.end_line),
        (3, 4, 6),
        "doc comment included"
    );
    let d = get("src/lib.rs", "Greeter", DefKind::Impl);
    assert_eq!((d.start_line, d.line, d.end_line), (12, 12, 20));
    let d = get("src/lib.rs", "greet", DefKind::Method);
    assert_eq!(
        (d.start_line, d.line, d.end_line, d.scope.as_deref()),
        (17, 17, 19, Some("Greeter"))
    );
    let d = get("backend/app.py", "greet", DefKind::Function);
    assert_eq!(
        (d.start_line, d.line, d.end_line),
        (15, 16, 17),
        "decorator included"
    );
    let d = get("backend/app.py", "users", DefKind::Table);
    assert_eq!(
        (d.start_line, d.line, d.end_line),
        (22, 22, 24),
        "DDL inside a Python string"
    );
    let d = get("frontend/app.js", "Dashboard", DefKind::Function);
    assert_eq!((d.start_line, d.line, d.end_line), (7, 7, 11));
    let d = get("svc/main.go", "Greet", DefKind::Method);
    assert_eq!(
        (d.start_line, d.line, d.end_line, d.scope.as_deref()),
        (5, 5, 5, Some("Greeter"))
    );
    let d = get("db/schema.sql", "greetings", DefKind::Table);
    assert_eq!((d.start_line, d.line, d.end_line), (1, 1, 4));
    let d = get("Cargo.toml", "greet", DefKind::Key);
    assert_eq!(
        (d.start_line, d.line, d.end_line, d.scope.as_deref()),
        (5, 5, 7, Some("dependencies"))
    );
    let d = get("docker-compose.yml", "greet", DefKind::Key);
    assert_eq!(
        (d.start_line, d.line, d.end_line, d.scope.as_deref()),
        (2, 2, 5, Some("services"))
    );
    let d = get("package.json", "scripts", DefKind::Key);
    assert_eq!((d.start_line, d.line, d.end_line), (3, 3, 5));
    let d = get("README.md", "How to greet", DefKind::Heading);
    assert_eq!((d.start_line, d.line, d.end_line), (5, 5, 11));
}

#[test]
fn ranking_exact_before_case_insensitive_before_contains_and_code_before_config() {
    let t = fixture();
    let r = find(t.path(), "greet", 100, &FindOptions::default()).unwrap();
    let kinds: Vec<MatchKind> = r.hits.iter().map(|h| h.matched).collect();
    assert!(kinds.windows(2).all(|w| w[0] <= w[1]), "{kinds:?}");
    let exact: Vec<&crate::Hit> = r
        .hits
        .iter()
        .filter(|h| h.matched == MatchKind::Exact)
        .collect();
    let first_config = exact
        .iter()
        .position(|h| matches!(h.def.kind, DefKind::Key | DefKind::Section));
    let last_code = exact.iter().rposition(|h| {
        !matches!(
            h.def.kind,
            DefKind::Key | DefKind::Section | DefKind::Heading
        )
    });
    assert!(
        last_code < first_config,
        "code before config: {:?}",
        exact
            .iter()
            .map(|h| (&h.def.path, h.def.kind))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        r.hits[0].def.path, "backend/app.py",
        "paths sort within a tier"
    );
}

#[test]
fn scoped_queries() {
    let t = fixture();
    let r = find(t.path(), "Greeter::greet", 100, &FindOptions::default()).unwrap();
    assert!(!r.hits.is_empty());
    assert!(
        r.hits
            .iter()
            .all(|h| h.def.scope.as_deref() == Some("Greeter")),
        "{:?}",
        r.hits
    );
    assert_eq!(r.scope.as_deref(), Some("Greeter"));
    assert!(r.caveats.iter().all(|c| !c.contains("no definition of")));

    let r = find(t.path(), "services.greet", 100, &FindOptions::default()).unwrap();
    assert_eq!(r.hits.len(), 1);
    assert_eq!(r.hits[0].def.path, "docker-compose.yml");

    let r = find(t.path(), "Nope::greet", 100, &FindOptions::default()).unwrap();
    assert!(!r.hits.is_empty(), "falls back to the bare name");
    assert_eq!(r.scope, None);
    assert!(
        r.caveats
            .iter()
            .any(|c| c.contains("no definition of `greet` inside a scope matching `Nope`")),
        "{:?}",
        r.caveats
    );
}

#[test]
fn limit_truncates_and_says_so() {
    let t = fixture();
    let all = find(t.path(), "greet", 100, &FindOptions::default()).unwrap();
    assert!(!all.truncated);
    assert_eq!(all.matched_total, all.hits.len());
    let some = find(t.path(), "greet", 2, &FindOptions::default()).unwrap();
    assert!(some.truncated);
    assert_eq!(some.hits.len(), 2);
    assert_eq!(some.matched_total, all.matched_total);
    assert_eq!(some.hits, all.hits[..2].to_vec());
    assert!(
        some.caveats
            .iter()
            .any(|c| c.contains(&format!("showing 2 of {}", all.matched_total)))
    );
}

#[test]
fn short_symbols_skip_substring_matching_and_say_so() {
    let t = fixture();
    let r = find(t.path(), "id", 100, &FindOptions::default()).unwrap();
    assert!(r.hits.iter().all(|h| h.matched != MatchKind::Contains));
    assert!(
        r.caveats.iter().any(|c| c.contains("shorter than 3")),
        "{:?}",
        r.caveats
    );
}

#[test]
fn user_errors() {
    let t = fixture();
    let e = find(t.path(), "   ", 10, &FindOptions::default()).unwrap_err();
    assert_eq!(e.exit_code(), 1, "{e}");
    let e = find(t.path(), "greet", 0, &FindOptions::default()).unwrap_err();
    assert_eq!(e.exit_code(), 1, "{e}");
    let e = find(
        &t.path().join("does-not-exist"),
        "greet",
        10,
        &FindOptions::default(),
    )
    .unwrap_err();
    assert!(matches!(e, cyberbrain_core::Error::Io { .. }), "{e}");
    let e = find(
        &t.path().join("Cargo.toml"),
        "greet",
        10,
        &FindOptions::default(),
    )
    .unwrap_err();
    assert!(e.to_string().contains("not a directory"), "{e}");
}

#[test]
fn malformed_ignore_file_is_refused_not_skipped() {
    let t = fixture();
    // `[unclosed` is accepted as a literal by the ignore crate; an unclosed `{` is not.
    t.write(IGNORE_FILE, "vendor/\n{unclosed\n");
    let e = find(t.path(), "greet", 10, &FindOptions::default()).unwrap_err();
    assert_eq!(e.exit_code(), 1);
    assert!(e.to_string().contains(IGNORE_FILE), "{e}");
}

#[test]
fn nested_ignore_files_and_whitelists() {
    let t = fixture();
    t.write("vendor/keep/lib.rs", "pub fn kept() {}\n");
    t.write("src/gen/out.rs", "pub fn generated() {}\n");
    t.write("src/.cyberbrainignore", "gen/\n");
    t.write(IGNORE_FILE, "vendor/*\n!vendor/keep\n");
    let r = find(t.path(), "kept", 10, &FindOptions::default()).unwrap();
    assert_eq!(r.hits.len(), 1, "{:?}", r.hits);
    let r = find(t.path(), "generated", 10, &FindOptions::default()).unwrap();
    assert!(r.hits.is_empty(), "{:?}", r.hits);
    assert!(
        r.ignore_files
            .contains(&"src/.cyberbrainignore".to_string()),
        "{:?}",
        r.ignore_files
    );
}

#[test]
fn excluded_store_and_gitignore_switch() {
    let t = fixture();
    t.write(".cyberbrain/notes/r2/note.md", "# greet\n");
    let mut opts = FindOptions {
        include_hidden: true,
        exclude: vec![t.path().join(".cyberbrain")],
        ..FindOptions::default()
    };
    let r = find(t.path(), "greet", 100, &opts).unwrap();
    assert_eq!(r.skipped.excluded_entries, 1);
    assert!(
        r.hits
            .iter()
            .all(|h| !h.def.path.starts_with(".cyberbrain"))
    );
    assert!(
        r.hits.iter().any(|h| h.def.path.starts_with(".hidden/")),
        "hidden included on request"
    );

    opts.honour_gitignore = false;
    let r = find(t.path(), "greet", 100, &opts).unwrap();
    assert!(r.hits.iter().any(|h| h.def.path.starts_with("build/")));
    assert_eq!(r.skipped.gitignored_entries, 0);
}

#[test]
fn never_stale_a_changed_file_is_seen_on_the_next_call() {
    let t = fixture();
    let before = find(t.path(), "Greeter::new", 10, &FindOptions::default()).unwrap();
    let line_before = before
        .hits
        .iter()
        .find(|h| h.def.path == "src/lib.rs")
        .unwrap()
        .def
        .line;
    t.write("src/lib.rs", format!("// one\n// two\n// three\n{RUST}"));
    let after = find(t.path(), "Greeter::new", 10, &FindOptions::default()).unwrap();
    let line_after = after
        .hits
        .iter()
        .find(|h| h.def.path == "src/lib.rs")
        .unwrap()
        .def
        .line;
    assert_eq!(line_after, line_before + 3);
}

#[test]
fn language_detection_by_extension() {
    for (f, l) in [
        ("a.rs", Language::Rust),
        ("a.PY", Language::Python),
        ("a.tsx", Language::TypeScript),
        ("a.mjs", Language::JavaScript),
        ("a.yml", Language::Yaml),
        ("a.markdown", Language::Markdown),
    ] {
        assert_eq!(Language::of_path(Path::new(f)), Some(l));
    }
    assert_eq!(Language::of_path(Path::new("Makefile")), None);
    assert_eq!(Language::of_path(Path::new("a.java")), None);
}

/// Measurement for the on-the-fly decision. `CB_FIND_BENCH_ROOT=<dir> cargo test -p
/// cyberbrain-code --release -- --ignored --nocapture bench`.
#[test]
#[ignore]
fn bench_scan_of_a_real_tree() {
    let Some(root) = std::env::var_os("CB_FIND_BENCH_ROOT") else {
        eprintln!("set CB_FIND_BENCH_ROOT");
        return;
    };
    let root = PathBuf::from(root);
    let opts = FindOptions::default();
    let mut times = Vec::new();
    let mut last = None;
    for _ in 0..7 {
        let s = scan(&root, &opts).unwrap();
        times.push(s.elapsed);
        last = Some(s);
    }
    let s = last.unwrap();
    times.sort();
    eprintln!(
        "root={} files_scanned={} bytes={} definitions={} skipped={:?}\nfirst={:?} min={:?} median={:?} max={:?}",
        s.root.display(),
        s.files_scanned,
        s.bytes_scanned,
        s.definitions.len(),
        (
            s.skipped.ignored_entries,
            s.skipped.gitignored_entries,
            s.skipped.hidden_entries,
            s.skipped.too_large,
            s.skipped.binary,
            s.skipped.unsupported,
            s.skipped.lockfiles
        ),
        times[0],
        times[0],
        times[times.len() / 2],
        times[times.len() - 1]
    );
}
