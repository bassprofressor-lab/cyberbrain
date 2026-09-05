//! The `---`-fenced YAML head of a note (SPEC §3.1): parse it, validate it, render it back.
//!
//! Rules this module enforces:
//!
//! - A malformed head is `Error::Frontmatter` naming the file and the reason. Never a
//!   panic, never a silent default. Unknown keys are rejected too: `tag:` instead of
//!   `tags:` would otherwise parse cleanly into an empty list and nobody would notice.
//! - Rendering writes fields in the order the spec lists them, which is the declaration
//!   order of [`Frontmatter`], so a parse → render round-trip is byte-stable.
//! - `name` is a kebab-case slug. It doubles as the file name inside the ring directory,
//!   so anything that is not a slug is rejected here before it can become a path.

use crate::error::{Error, Result};
use crate::types::Frontmatter;
use std::path::Path;

/// The fence line. Exactly three dashes, nothing else on the line.
const FENCE: &str = "---";

/// Keys the head may contain. Anything else is a typo or a field from another tool.
const KNOWN_KEYS: [&str; 10] = [
    "id",
    "name",
    "ring",
    "kind",
    "created",
    "updated",
    "tags",
    "links",
    "retention",
    "pii",
];

/// A note file split into its two halves. `body` borrows from the input and starts after
/// the closing fence and at most one blank line, so it begins with content.
#[derive(Debug, Clone)]
pub struct Parsed<'a> {
    pub front: Frontmatter,
    pub body: &'a str,
}

/// Parse a whole note file. `path` is only used to name the file in errors.
pub fn parse<'a>(path: &Path, text: &'a str) -> Result<Parsed<'a>> {
    let fail = |reason: String| Error::Frontmatter {
        path: path.to_path_buf(),
        reason,
    };

    // A UTF-8 BOM before the fence is common when a file went through Windows tooling;
    // it is not part of the head.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);

    let (head, body) = split_fences(text).map_err(|r| fail(r.to_string()))?;

    // First pass: a generic mapping, so unknown keys and the wrong top-level shape get a
    // precise message instead of serde's "missing field" for something unrelated.
    let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(head)
        .map_err(|e| fail(format!("{} (in the YAML head, {})", e, head_location(&e))))?;
    let mapping = match value {
        serde_yaml_ng::Value::Mapping(m) => m,
        serde_yaml_ng::Value::Null => return Err(fail("the head is empty".to_string())),
        other => {
            return Err(fail(format!(
                "the head must be a mapping of key: value pairs, found {}",
                yaml_kind(&other)
            )));
        }
    };
    for (key, value) in &mapping {
        match key {
            serde_yaml_ng::Value::String(k) if KNOWN_KEYS.contains(&k.as_str()) => {
                check_field(k, value).map_err(|why| fail(format!("field `{k}`: {why}")))?;
            }
            serde_yaml_ng::Value::String(k) => {
                return Err(fail(format!(
                    "unknown field `{k}`; known fields are {}",
                    KNOWN_KEYS.join(", ")
                )));
            }
            other => {
                return Err(fail(format!(
                    "keys must be plain strings, found {}",
                    yaml_kind(other)
                )));
            }
        }
    }

    // Second pass: the typed struct. Every field-level error from serde already names the
    // field ("missing field `ring`", "ring 7 does not exist").
    let front: Frontmatter = serde_yaml_ng::from_value(serde_yaml_ng::Value::Mapping(mapping))
        .map_err(|e| fail(e.to_string()))?;

    validate_name(&front.name).map_err(|r| fail(format!("name `{}`: {r}", front.name)))?;
    if let Some(r) = &front.retention {
        validate_retention(r).map_err(|why| fail(format!("retention `{r}`: {why}")))?;
    }

    Ok(Parsed { front, body })
}

/// Render a head and body back into file text. Inverse of [`parse`]: the output starts
/// with the fence, ends the head with a fence, then one blank line, then the body,
/// byte for byte — the body is the human's text and is not normalised here.
pub fn render(front: &Frontmatter, body: &str) -> Result<String> {
    validate_name(&front.name).map_err(|r| Error::Frontmatter {
        path: Path::new(&format!("{}.md", front.name)).to_path_buf(),
        reason: format!("name `{}`: {r}", front.name),
    })?;
    let yaml = serde_yaml_ng::to_string(front).map_err(|e| Error::Frontmatter {
        path: Path::new(&format!("{}.md", front.name)).to_path_buf(),
        reason: format!("cannot serialise: {e}"),
    })?;
    let mut out = String::with_capacity(yaml.len() + body.len() + 16);
    out.push_str(FENCE);
    out.push('\n');
    out.push_str(&yaml);
    if !yaml.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(FENCE);
    out.push('\n');
    out.push('\n');
    out.push_str(body);
    Ok(out)
}

/// Why a string is not an acceptable note name, or `Ok` if it is.
///
/// A name is a kebab-case slug: ASCII lowercase letters and digits, single hyphens
/// between them, 1..=120 characters. It becomes `<name>.md` inside a ring directory, so
/// the rule also rules out every path-traversal shape (`..`, `/`, `\`, drive letters).
pub fn validate_name(name: &str) -> std::result::Result<(), &'static str> {
    if name.is_empty() {
        return Err("must not be empty");
    }
    if name.len() > 120 {
        return Err("must be at most 120 characters");
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Err("must not start or end with a hyphen");
    }
    if name.contains("--") {
        return Err("must not contain consecutive hyphens");
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err("must be a kebab-case slug: lowercase ascii letters, digits and hyphens");
    }
    Ok(())
}

/// ISO-8601 duration of the calendar-ish shape used in the spec (`P2Y`, `P30D`, `PT12H`,
/// `P1Y6M`). Only the shape is checked here; the policy layer interprets it.
pub fn validate_retention(s: &str) -> std::result::Result<(), &'static str> {
    let rest = s.strip_prefix('P').ok_or("must start with `P`")?;
    if rest.is_empty() {
        return Err("has no components after `P`");
    }
    let (date, time) = match rest.split_once('T') {
        Some((d, t)) => (d, Some(t)),
        None => (rest, None),
    };
    if time == Some("") {
        return Err("has a `T` with no time components after it");
    }
    let check = |part: &str, allowed: &str| -> std::result::Result<(), &'static str> {
        let mut digits = 0usize;
        // Position of the last unit seen inside `allowed`; units must strictly advance.
        let mut last_unit: Option<usize> = None;
        for ch in part.chars() {
            if ch.is_ascii_digit() {
                digits += 1;
            } else if let Some(unit) = allowed.find(ch) {
                if digits == 0 {
                    return Err("a unit letter must follow a number");
                }
                if last_unit.is_some_and(|prev| unit <= prev) {
                    return Err("units must appear in descending order without repeats");
                }
                last_unit = Some(unit);
                digits = 0;
            } else {
                return Err("contains a character that is neither a digit nor a unit");
            }
        }
        if digits != 0 {
            return Err("ends with a number that has no unit");
        }
        Ok(())
    };
    check(date, "YMWD")?;
    if let Some(t) = time {
        check(t, "HMS")?;
    }
    Ok(())
}

/// Locate the fences and return `(yaml, body)`. Handles CRLF by accepting `---\r` as a
/// fence line; the YAML parser copes with `\r\n` inside the head on its own.
fn split_fences(text: &str) -> std::result::Result<(&str, &str), &'static str> {
    let mut lines = text.split_inclusive('\n');
    let first = lines.next().ok_or("file is empty")?;
    if first.trim_end_matches(['\r', '\n']) != FENCE {
        return Err("must start with a `---` line");
    }
    let head_start = first.len();
    let mut pos = head_start;
    for line in lines {
        if line.trim_end_matches(['\r', '\n']) == FENCE {
            let head = &text[head_start..pos];
            let mut body = &text[pos + line.len()..];
            // One blank line after the closing fence is layout, not body.
            body = body
                .strip_prefix("\r\n")
                .or_else(|| body.strip_prefix('\n'))
                .unwrap_or(body);
            return Ok((head, body));
        }
        pos += line.len();
    }
    Err("the head is never closed by a second `---` line")
}

/// Deserialise one field into its type so a bad value is reported under its own name.
/// `serde_yaml_ng::from_value` on the whole struct loses the field path for errors that
/// come from inside a type's own parser (a bad timestamp says "failed to parse four
/// digit integer as year" and nothing else).
fn check_field(key: &str, value: &serde_yaml_ng::Value) -> std::result::Result<(), String> {
    use crate::types::{NoteId, NoteKind, PiiState, Ring};
    fn as_<T: serde::de::DeserializeOwned>(
        v: &serde_yaml_ng::Value,
    ) -> std::result::Result<(), String> {
        serde_yaml_ng::from_value::<T>(v.clone())
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    match key {
        "id" => as_::<NoteId>(value),
        "name" => as_::<String>(value),
        "ring" => as_::<Ring>(value),
        "kind" => as_::<NoteKind>(value),
        "created" | "updated" => as_::<jiff::Timestamp>(value),
        "tags" | "links" => as_::<Vec<String>>(value),
        "retention" => as_::<Option<String>>(value),
        "pii" => as_::<PiiState>(value),
        _ => Ok(()),
    }
}

fn head_location(e: &serde_yaml_ng::Error) -> String {
    match e.location() {
        // +1: the head starts on line 2 of the file, after the opening fence.
        Some(loc) => format!("file line {}, column {}", loc.line() + 1, loc.column()),
        None => "position unknown".to_string(),
    }
}

fn yaml_kind(v: &serde_yaml_ng::Value) -> &'static str {
    match v {
        serde_yaml_ng::Value::Null => "null",
        serde_yaml_ng::Value::Bool(_) => "a boolean",
        serde_yaml_ng::Value::Number(_) => "a number",
        serde_yaml_ng::Value::String(_) => "a string",
        serde_yaml_ng::Value::Sequence(_) => "a list",
        serde_yaml_ng::Value::Mapping(_) => "a mapping",
        serde_yaml_ng::Value::Tagged(_) => "a tagged value",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{NoteKind, PiiState, Ring};

    const SPEC_EXAMPLE: &str = "\
---
id: 01ARZ3NDEKTSV4RRFFQ69G5FAV
name: pg18-moves-pgdata
ring: 2
kind: knowledge
created: 2026-09-05T09:12:03Z
updated: 2026-09-05T09:12:03Z
tags:
- postgres
- deployment
links:
- docker-bind-mount-inode-drift
retention: P2Y
pii: none
---

Body in Markdown. Links to other notes are written [[like-this]].
";

    fn p() -> &'static Path {
        Path::new("notes/r2/pg18-moves-pgdata.md")
    }

    #[test]
    fn parses_the_spec_example() {
        let parsed = parse(p(), SPEC_EXAMPLE).unwrap();
        let f = &parsed.front;
        assert_eq!(f.id.to_string(), "01ARZ3NDEKTSV4RRFFQ69G5FAV");
        assert_eq!(f.name, "pg18-moves-pgdata");
        assert_eq!(f.ring, Ring::Knowledge);
        assert_eq!(f.kind, NoteKind::Knowledge);
        assert_eq!(f.created.to_string(), "2026-09-05T09:12:03Z");
        assert_eq!(f.tags, ["postgres", "deployment"]);
        assert_eq!(f.links, ["docker-bind-mount-inode-drift"]);
        assert_eq!(f.retention.as_deref(), Some("P2Y"));
        assert_eq!(f.pii, PiiState::None);
        assert_eq!(
            parsed.body,
            "Body in Markdown. Links to other notes are written [[like-this]].\n"
        );
    }

    #[test]
    fn round_trip_is_byte_stable_and_keeps_spec_field_order() {
        let parsed = parse(p(), SPEC_EXAMPLE).unwrap();
        let rendered = render(&parsed.front, parsed.body).unwrap();
        assert_eq!(rendered, SPEC_EXAMPLE);
        let again = parse(p(), &rendered).unwrap();
        assert_eq!(again.body, parsed.body);
        assert_eq!(
            serde_yaml_ng::to_string(&again.front).unwrap(),
            serde_yaml_ng::to_string(&parsed.front).unwrap()
        );

        // Field order as the spec lists it.
        let head_keys: Vec<&str> = rendered
            .lines()
            .skip(1)
            .take_while(|l| *l != FENCE)
            .filter(|l| !l.starts_with(['-', ' ']))
            .map(|l| l.split(':').next().unwrap())
            .collect();
        assert_eq!(
            head_keys,
            [
                "id",
                "name",
                "ring",
                "kind",
                "created",
                "updated",
                "tags",
                "links",
                "retention",
                "pii"
            ]
        );
    }

    #[test]
    fn optional_fields_may_be_absent_and_are_not_written_when_empty() {
        let text = "---\nid: 01ARZ3NDEKTSV4RRFFQ69G5FAV\nname: a\nring: 0\nkind: bug\n\
                    created: 2026-09-05T09:12:03Z\nupdated: 2026-09-05T09:12:03Z\n---\nbody";
        let parsed = parse(p(), text).unwrap();
        assert!(parsed.front.tags.is_empty());
        assert!(parsed.front.links.is_empty());
        assert_eq!(parsed.front.retention, None);
        // An absent `pii:` means nobody scanned, NOT that a scan came back clean. Reading it
        // as `None` would hand the compliance layer a clean bill of health no scan issued.
        assert_eq!(parsed.front.pii, PiiState::Unscanned);
        assert!(!parsed.front.pii.was_scanned());
        assert_eq!(parsed.body, "body");
        let rendered = render(&parsed.front, parsed.body).unwrap();
        assert!(!rendered.contains("tags"), "{rendered}");
        assert!(!rendered.contains("links"), "{rendered}");
        assert!(!rendered.contains("retention"), "{rendered}");
        // pii is always written, and it round-trips as `unscanned` rather than collapsing
        // into `none`: "nobody looked" and "looked, found nothing" are different facts and
        // the compliance layer acts differently on each.
        assert!(rendered.contains("pii: unscanned"), "{rendered}");
    }

    #[test]
    fn accepts_crlf_and_bom() {
        let text = SPEC_EXAMPLE.replace('\n', "\r\n");
        let text = format!("\u{feff}{text}");
        let parsed = parse(p(), &text).unwrap();
        assert_eq!(parsed.front.name, "pg18-moves-pgdata");
        assert!(parsed.body.starts_with("Body in Markdown"));
    }

    #[test]
    fn body_keeps_leading_content_after_one_blank_line() {
        let text = "---\nid: 01ARZ3NDEKTSV4RRFFQ69G5FAV\nname: a\nring: 2\nkind: lesson\n\
                    created: 2026-09-05T09:12:03Z\nupdated: 2026-09-05T09:12:03Z\n---\n\n\n# two blanks\n";
        let parsed = parse(p(), text).unwrap();
        assert_eq!(parsed.body, "\n# two blanks\n");
    }

    #[test]
    fn head_can_be_the_entire_file() {
        let text = "---\nid: 01ARZ3NDEKTSV4RRFFQ69G5FAV\nname: a\nring: 2\nkind: lesson\n\
                    created: 2026-09-05T09:12:03Z\nupdated: 2026-09-05T09:12:03Z\n---";
        let parsed = parse(p(), text).unwrap();
        assert_eq!(parsed.body, "");
    }

    fn expect_reason(text: &str, needle: &str) {
        match parse(p(), text) {
            Err(Error::Frontmatter { path, reason }) => {
                assert_eq!(path, p());
                assert!(
                    reason.contains(needle),
                    "reason {reason:?} lacks {needle:?}"
                );
            }
            other => panic!("expected a frontmatter error containing {needle:?}, got {other:?}"),
        }
    }

    #[test]
    fn malformed_heads_name_the_file_and_the_reason() {
        expect_reason("", "empty");
        expect_reason("no fence\n", "must start with");
        expect_reason("---\nid: x\n", "never closed");
        expect_reason("---\n---\nbody", "head is empty");
        expect_reason("---\n- a\n- b\n---\n", "must be a mapping");
        expect_reason("---\nid: [unclosed\n---\n", "YAML head");
        expect_reason(
            "---\nid: 01ARZ3NDEKTSV4RRFFQ69G5FAV\nname: a\nkind: bug\n\
             created: 2026-09-05T09:12:03Z\nupdated: 2026-09-05T09:12:03Z\n---\n",
            "missing field `ring`",
        );
        expect_reason(
            "---\nid: 01ARZ3NDEKTSV4RRFFQ69G5FAV\nname: a\nring: 7\nkind: bug\n\
             created: 2026-09-05T09:12:03Z\nupdated: 2026-09-05T09:12:03Z\n---\n",
            "ring 7 does not exist",
        );
        expect_reason(
            "---\nid: 01ARZ3NDEKTSV4RRFFQ69G5FAV\nname: a\nring: 2\nkind: poem\n\
             created: 2026-09-05T09:12:03Z\nupdated: 2026-09-05T09:12:03Z\n---\n",
            "poem",
        );
        expect_reason(
            "---\nid: not-a-ulid\nname: a\nring: 2\nkind: bug\n\
             created: 2026-09-05T09:12:03Z\nupdated: 2026-09-05T09:12:03Z\n---\n",
            "id",
        );
        expect_reason(
            "---\nid: 01ARZ3NDEKTSV4RRFFQ69G5FAV\nname: a\nring: 2\nkind: bug\n\
             created: yesterday\nupdated: 2026-09-05T09:12:03Z\n---\n",
            "created",
        );
    }

    #[test]
    fn unknown_keys_are_an_error_not_a_silent_default() {
        expect_reason(
            "---\nid: 01ARZ3NDEKTSV4RRFFQ69G5FAV\nname: a\nring: 2\nkind: bug\n\
             created: 2026-09-05T09:12:03Z\nupdated: 2026-09-05T09:12:03Z\ntag: [x]\n---\n",
            "unknown field `tag`",
        );
    }

    #[test]
    fn bad_names_are_rejected() {
        for bad in [
            "",
            "Has-Upper",
            "under_score",
            "../escape",
            "sub/dir",
            "back\\slash",
            "-leading",
            "trailing-",
            "double--hyphen",
            "space here",
            "ümlaut",
        ] {
            assert!(validate_name(bad).is_err(), "{bad:?} should be rejected");
        }
        for good in ["a", "pg18-moves-pgdata", "bug-231", "x1-y2-z3"] {
            assert!(validate_name(good).is_ok(), "{good:?} should be accepted");
        }
        expect_reason(
            "---\nid: 01ARZ3NDEKTSV4RRFFQ69G5FAV\nname: ../escape\nring: 2\nkind: bug\n\
             created: 2026-09-05T09:12:03Z\nupdated: 2026-09-05T09:12:03Z\n---\n",
            "name `../escape`",
        );
    }

    #[test]
    fn retention_shape_is_checked() {
        for good in ["P2Y", "P30D", "PT12H", "P1Y6M", "P1W", "P1DT2H30M", "P0D"] {
            assert!(validate_retention(good).is_ok(), "{good}");
        }
        for bad in [
            "", "2Y", "P", "PT", "PY", "P2", "P2X", "P1M2Y", "P2Y1Y", "P1D2H",
        ] {
            assert!(validate_retention(bad).is_err(), "{bad}");
        }
        expect_reason(
            "---\nid: 01ARZ3NDEKTSV4RRFFQ69G5FAV\nname: a\nring: 2\nkind: bug\n\
             created: 2026-09-05T09:12:03Z\nupdated: 2026-09-05T09:12:03Z\nretention: 2 years\n---\n",
            "retention `2 years`",
        );
    }

    #[test]
    fn render_refuses_a_bad_name() {
        let parsed = parse(p(), SPEC_EXAMPLE).unwrap();
        let mut front = parsed.front;
        front.name = "Not A Slug".into();
        assert!(matches!(render(&front, ""), Err(Error::Frontmatter { .. })));
    }
}
