//! `serde` adapters for [`crate::slash`].
//!
//! A `PathBuf` field of a report keeps its type, because a caller may open it; but a
//! derived `Serialize` would write it with the platform separator, and the JSON goes to
//! `--json`, the HTTP API and MCP `structuredContent`, where it is compared across
//! machines. So the field stays a `PathBuf` and only its serialisation changes:
//!
//! ```ignore
//! #[serde(serialize_with = "cyberbrain_core::path_serde::slash")]
//! pub store: PathBuf,
//! #[serde(serialize_with = "cyberbrain_core::path_serde::slash_opt")]
//! pub artefact_path: Option<PathBuf>,
//! ```
//!
//! Deserialisation is untouched: a path read back with forward slashes opens on every
//! platform.

use serde::Serializer;
use std::path::Path;

/// Serialise a path as a string with forward slashes.
pub fn slash<P: AsRef<Path> + ?Sized, S: Serializer>(p: &P, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&crate::path::slash(p.as_ref()))
}

/// [`slash`] for an `Option`; `None` stays `null`.
pub fn slash_opt<P: AsRef<Path>, S: Serializer>(p: &Option<P>, s: S) -> Result<S::Ok, S::Error> {
    match p {
        Some(p) => s.serialize_some(&crate::path::slash(p.as_ref())),
        None => s.serialize_none(),
    }
}

#[cfg(test)]
mod tests {
    use serde::Serialize;
    use std::path::PathBuf;

    #[derive(Serialize)]
    struct Report {
        #[serde(serialize_with = "super::slash")]
        path: PathBuf,
        #[serde(serialize_with = "super::slash_opt")]
        maybe: Option<PathBuf>,
        #[serde(serialize_with = "super::slash_opt")]
        absent: Option<PathBuf>,
    }

    #[test]
    fn a_path_field_serialises_with_forward_slashes() {
        let r = Report {
            path: ["notes", "r2", "a.md"].iter().collect(),
            maybe: Some(["models", "m"].iter().collect()),
            absent: None,
        };
        // YAML rather than JSON: the core crate has no serde_json, and the property is
        // the serialiser-independent string the adapter hands over.
        let text = serde_yaml_ng::to_string(&r).unwrap();
        assert_eq!(text, "path: notes/r2/a.md\nmaybe: models/m\nabsent: null\n");
    }
}
