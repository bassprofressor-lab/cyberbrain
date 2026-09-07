//! Messages must read like sentences, not like the source they were written in.
//!
//! `rustfmt` collapses a multi-line `format!` string onto one line and keeps the
//! continuation's indentation as literal spaces. The result compiles, the test suite stays
//! green, and a user sees
//!
//! ```text
//! no egress gate is                  wired in
//! ```
//!
//! That exact line shipped. It happened three more times in one afternoon while the hub was
//! being written, which is the point at which a check is cheaper than the next discovery.

use std::fs;
use std::path::{Path, PathBuf};

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            let name = p.file_name().unwrap().to_string_lossy().to_string();
            if name == "target" || name == "node_modules" || name.starts_with('.') {
                continue;
            }
            walk(&p, out);
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
}

/// Runs of four or more spaces inside a string literal, ignoring the ones that are
/// deliberate: after an escape (`\n`, `\t`), and in a line that lines up columns with a
/// format specifier such as `{:<14}`.
fn suspicious_runs(line: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    let mut in_string = false;
    let mut current = String::new();
    while i < chars.len() {
        let c = chars[i];
        if in_string {
            if c == '\\' && i + 1 < chars.len() {
                current.push(c);
                current.push(chars[i + 1]);
                i += 2;
                continue;
            }
            if c == '"' {
                in_string = false;
                // A literal that aligns columns is allowed to hold runs of spaces.
                let aligns = current.contains("{:");
                if !aligns {
                    for part in current.split("\\n") {
                        if let Some(pos) = part.find("    ") {
                            let before = part[..pos].chars().last();
                            let after = part[pos..].trim_start().chars().next();
                            // Only flag a run that sits between two words: that is the shape
                            // of a sentence that was wrapped in the source.
                            if before.is_some_and(|c| c.is_alphanumeric() || c == ',' || c == ';')
                                && after.is_some_and(|c| c.is_alphanumeric())
                            {
                                found.push(current.clone());
                                break;
                            }
                        }
                    }
                }
                current.clear();
            } else {
                current.push(c);
            }
        } else if c == '"' {
            in_string = true;
            current.clear();
        }
        i += 1;
    }
    let _ = &mut chars;
    found
}

#[test]
fn no_message_carries_the_indentation_it_was_written_with() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let mut files = Vec::new();
    walk(&root.join("crates"), &mut files);
    assert!(files.len() > 5, "the walk found nothing; is it broken?");

    let mut offences = Vec::new();
    for f in &files {
        let Ok(text) = fs::read_to_string(f) else {
            continue;
        };
        for (n, line) in text.lines().enumerate() {
            for s in suspicious_runs(line) {
                offences.push(format!(
                    "{}:{}\n    {}",
                    f.strip_prefix(&root).unwrap_or(f).display(),
                    n + 1,
                    s.chars().take(110).collect::<String>()
                ));
            }
        }
    }

    assert!(
        offences.is_empty(),
        "a message carries the indentation of the source it was written in — rustfmt \
         collapsed a multi-line string. Use concat!(\"a \", \"b\") instead of a backslash \
         continuation:\n\n{}\n",
        offences.join("\n")
    );
}

/// The check has to fail on the shape it exists for, or it is decoration.
#[test]
fn the_check_catches_the_shape_it_is_for() {
    let bad = r#"    let m = "no egress gate is                  wired in; a wiring bug";"#;
    assert_eq!(suspicious_runs(bad).len(), 1, "should have flagged it");

    // And leaves the deliberate ones alone.
    for ok in [
        r#"    let m = "def f():\n    x = 1\n";"#,
        r#"    format!("{:<14} {:<28}", state, name)"#,
        r#"    let m = "search it:      cyberbrain recall";"#,
    ] {
        assert!(suspicious_runs(ok).is_empty(), "false positive on {ok}");
    }
}
