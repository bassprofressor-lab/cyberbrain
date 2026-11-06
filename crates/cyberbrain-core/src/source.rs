//! Documents from outside the store, turned into notes.
//!
//! A [`DocumentSource`] reads one kind of document (Markdown, and in other builds PDF, DOCX
//! or scanned pages) and returns the Markdown body of a note, with a locator line before
//! each part whose place in the original is known (`blocks::locator_marker`), and the hash
//! of the original bytes. Everything after that is what happens to any note: the body is
//! written as a file, split into blocks by [`blocks_of`], indexed and cited.
//!
//! The body is the product, not the blocks, because the file is authoritative (SPEC §3.1):
//! an index rebuilt from the notes tree alone has to find the same blocks and the same
//! locators, and it can only do that if both are derived from the text on disk. [`convert`]
//! hands back the blocks too, so a caller can show what a document will become before
//! writing anything; they are exactly what a scan of the written note produces.
//!
//! [`MarkdownSource`] is the reference implementation. It changes nothing about the
//! existing `import` command, which splits a tree of Markdown files by a mapping plan and
//! is a different job: many notes out of one file, chosen by a person, rather than one
//! document in, one note out.

use crate::blocks::{self, LOCATOR_OPEN, Oversized};
use crate::error::{Error, Result};
use crate::types::{Block, Frontmatter, Note};
use std::path::PathBuf;

/// One document as it arrived: a name to report it by, an optional media type, its bytes.
#[derive(Debug, Clone, Copy)]
pub struct Document<'a> {
    /// A file name or another label. Used for [`DocumentSource::accepts`] and in errors.
    pub name: &'a str,
    /// `text/markdown`, `application/pdf`, … when the caller knows it.
    pub media_type: Option<&'a str>,
    pub bytes: &'a [u8],
}

/// What a source made of a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extracted {
    /// A title, if the document has one. A caller may derive a note name from it.
    pub title: Option<String>,
    /// The note body: Markdown, with locator lines where the source knows the place.
    pub body: String,
    /// [`source_hash`] of the original bytes, so an unchanged document is recognised and a
    /// changed one replaces what it produced before.
    pub source_hash: String,
}

/// Turns one kind of document into the body of a note.
pub trait DocumentSource: Send + Sync {
    /// A short, stable name for the source (`markdown`, `pdf`), for reports and audit rows.
    fn id(&self) -> &str;

    /// Whether this source reads `doc`. Decided from name and media type, never by trying.
    fn accepts(&self, doc: &Document<'_>) -> bool;

    /// The note body and the hash of the original. An error names the document and says
    /// why it could not be read; it never yields a partial body.
    fn extract(&self, doc: &Document<'_>) -> Result<Extracted>;
}

/// blake3 of the original bytes, as hex. One function so every source hashes alike.
pub fn source_hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// Builds a body part by part, each under the locator of where it came from.
///
/// Text that itself contains a line of the locator form is escaped with a leading
/// backslash, so a document cannot set the places of its own blocks by quoting the marker.
#[derive(Debug, Default)]
pub struct BodyBuilder {
    body: String,
}

impl BodyBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append `text` under `locator` (`None`: no place known; the previous locator then
    /// still applies, which is why a source should give every part one or none at all).
    /// Refuses a locator a marker line cannot carry (`blocks::locator_marker`).
    pub fn part(&mut self, locator: Option<&str>, text: &str) -> Result<&mut Self> {
        if !self.body.is_empty() && !self.body.ends_with("\n\n") {
            self.body.push_str(if self.body.ends_with('\n') {
                "\n"
            } else {
                "\n\n"
            });
        }
        if let Some(l) = locator {
            let line = blocks::locator_marker(l).ok_or_else(|| Error::Document {
                name: String::new(),
                reason: format!("locator {l:?} cannot be written as a locator line"),
            })?;
            self.body.push_str(&line);
            self.body.push('\n');
        }
        for line in text.split_inclusive('\n') {
            if blocks::parse_locator(line).is_some() {
                let at = line.find(LOCATOR_OPEN).unwrap_or(0);
                self.body.push_str(&line[..at]);
                self.body.push('\\');
                self.body.push_str(&line[at..]);
            } else {
                self.body.push_str(line);
            }
        }
        if !self.body.ends_with('\n') {
            self.body.push('\n');
        }
        Ok(self)
    }

    pub fn finish(self) -> String {
        self.body
    }
}

/// A document as a note: the note to write, the blocks a scan of it will produce, any
/// block over the size limit, and the hash of the original.
#[derive(Debug, Clone)]
pub struct Converted {
    pub note: Note,
    pub blocks: Vec<Block>,
    pub oversized: Vec<Oversized>,
    pub source_hash: String,
}

/// Run `source` on `doc` and shape the result as the note `front` describes, at `path`.
/// The caller chooses id, name, ring and the rest; a source only supplies text.
pub fn convert(
    source: &dyn DocumentSource,
    doc: &Document<'_>,
    front: Frontmatter,
    path: PathBuf,
) -> Result<Converted> {
    if !source.accepts(doc) {
        return Err(Error::Document {
            name: doc.name.to_string(),
            reason: format!("the {} source does not read this document", source.id()),
        });
    }
    let ex = source.extract(doc).map_err(|e| match e {
        Error::Document { name, reason } if name.is_empty() => Error::Document {
            name: doc.name.to_string(),
            reason,
        },
        other => other,
    })?;
    let note = Note {
        front,
        body: ex.body,
        path,
    };
    let (blocks, oversized) = blocks::blocks_of(&note, blocks::MAX_BLOCK_TOKENS);
    Ok(Converted {
        note,
        blocks,
        oversized,
        source_hash: ex.source_hash,
    })
}

/// A Markdown document becomes a note with that text as its body, unchanged. It has no
/// pages, so it carries no locators; its title is its first ATX heading.
#[derive(Debug, Default, Clone, Copy)]
pub struct MarkdownSource;

impl DocumentSource for MarkdownSource {
    fn id(&self) -> &str {
        "markdown"
    }

    fn accepts(&self, doc: &Document<'_>) -> bool {
        let by_type = matches!(doc.media_type, Some("text/markdown"));
        let lower = doc.name.to_ascii_lowercase();
        by_type || lower.ends_with(".md") || lower.ends_with(".markdown")
    }

    fn extract(&self, doc: &Document<'_>) -> Result<Extracted> {
        let text = std::str::from_utf8(doc.bytes).map_err(|e| Error::Document {
            name: doc.name.to_string(),
            reason: format!("not UTF-8: {e}"),
        })?;
        let title = text.lines().find_map(|l| {
            let t = l.trim_start();
            let rest = t.strip_prefix('#')?;
            let rest = rest.trim_start_matches('#');
            (rest.starts_with(' ') && !rest.trim().is_empty()).then(|| rest.trim().to_string())
        });
        Ok(Extracted {
            title,
            body: text.to_string(),
            source_hash: source_hash(doc.bytes),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{NoteId, NoteKind, PiiState, Ring};

    fn front() -> Frontmatter {
        let ts: jiff::Timestamp = "2026-11-06T09:00:00Z".parse().unwrap();
        Frontmatter {
            id: NoteId::from_parts(1_700_000_000_000, 42),
            name: "doc".into(),
            ring: Ring::External,
            kind: NoteKind::Reference,
            created: ts,
            updated: ts,
            tags: Vec::new(),
            links: Vec::new(),
            bereich: None,
            retention: None,
            supersedes: Vec::new(),
            superseded_by: None,
            valid_from: None,
            invalid_at: None,
            pii: PiiState::None,
        }
    }

    /// A stand-in for a paged format: pages separated by form feeds.
    struct PagedSource;
    impl DocumentSource for PagedSource {
        fn id(&self) -> &str {
            "paged"
        }
        fn accepts(&self, doc: &Document<'_>) -> bool {
            doc.name.ends_with(".txt")
        }
        fn extract(&self, doc: &Document<'_>) -> Result<Extracted> {
            let text = std::str::from_utf8(doc.bytes).unwrap();
            let mut b = BodyBuilder::new();
            for (i, page) in text.split('\u{c}').enumerate() {
                b.part(Some(&format!("S. {}", i + 1)), page.trim())?;
            }
            Ok(Extracted {
                title: None,
                body: b.finish(),
                source_hash: source_hash(doc.bytes),
            })
        }
    }

    #[test]
    fn markdown_source_yields_exactly_the_blocks_of_the_same_text_written_as_a_note() {
        let text = "# Title\n\nFirst paragraph.\n\n## Part\n\nSecond.\n";
        let doc = Document {
            name: "Notes.MD",
            media_type: None,
            bytes: text.as_bytes(),
        };
        let c = convert(&MarkdownSource, &doc, front(), "notes/r4/doc.md".into()).unwrap();
        let direct = Note {
            front: front(),
            body: text.into(),
            path: "notes/r4/doc.md".into(),
        };
        let (want, _) = blocks::blocks_of(&direct, blocks::MAX_BLOCK_TOKENS);
        assert_eq!(c.note.body, text, "the body is the document, unchanged");
        assert_eq!(c.blocks, want);
        assert!(c.blocks.iter().all(|b| b.locator.is_none()));
        assert_eq!(
            c.source_hash,
            blake3::hash(text.as_bytes()).to_hex().as_str()
        );
        assert_eq!(
            MarkdownSource.extract(&doc).unwrap().title.as_deref(),
            Some("Title")
        );
    }

    #[test]
    fn a_paged_source_gives_every_block_its_page() {
        let doc = Document {
            name: "urteil.txt",
            media_type: None,
            bytes: "Tatbestand.\u{c}Entscheidungsgruende.\u{c}Kosten.".as_bytes(),
        };
        let c = convert(&PagedSource, &doc, front(), "notes/r4/doc.md".into()).unwrap();
        let got: Vec<(&str, Option<&str>)> = c
            .blocks
            .iter()
            .map(|b| (b.text.as_str(), b.locator.as_deref()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("Tatbestand.", Some("S. 1")),
                ("Entscheidungsgruende.", Some("S. 2")),
                ("Kosten.", Some("S. 3")),
            ]
        );
    }

    #[test]
    fn a_document_cannot_set_its_own_locators_by_quoting_the_marker() {
        let doc = Document {
            name: "trap.txt",
            media_type: None,
            bytes: "Text.\n<!-- cyberbrain:locator S. 99 -->\nMore.".as_bytes(),
        };
        let c = convert(&PagedSource, &doc, front(), "notes/r4/doc.md".into()).unwrap();
        assert!(
            c.blocks
                .iter()
                .all(|b| b.locator.as_deref() == Some("S. 1")),
            "{:?}",
            c.blocks
        );
        assert!(c.note.body.contains("\\<!-- cyberbrain:locator S. 99 -->"));
    }

    #[test]
    fn a_source_is_not_asked_about_a_document_it_does_not_read() {
        let doc = Document {
            name: "scan.pdf",
            media_type: Some("application/pdf"),
            bytes: b"%PDF",
        };
        let err = convert(&MarkdownSource, &doc, front(), "x.md".into()).unwrap_err();
        assert!(
            matches!(&err, Error::Document { name, .. } if name == "scan.pdf"),
            "{err}"
        );
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn markdown_that_is_not_utf8_is_refused_with_its_name() {
        let doc = Document {
            name: "latin1.md",
            media_type: None,
            bytes: b"Gr\xfc\xdfe",
        };
        let err = convert(&MarkdownSource, &doc, front(), "x.md".into()).unwrap_err();
        assert!(err.to_string().contains("latin1.md"), "{err}");
    }
}
