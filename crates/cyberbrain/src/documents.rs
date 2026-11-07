//! `cyberbrain documents`: PDFs and scans in, one note per document (feature `documents`).
//!
//! Reading is parallel, writing is not: every note goes through `App::write`, which is where
//! the PII hold, the audit row and the index update live. A document is not a second way into
//! the store with its own rules; it is a note whose body happens to come from a PDF.
//!
//! The libraries and models are looked up in the store's `models/` folder, next to model2vec:
//!   models/pdfium/        libpdfium.so | libpdfium.dylib | pdfium.dll
//!   models/onnxruntime/   libonnxruntime.so | libonnxruntime.dylib | onnxruntime.dll
//!   models/ppocrv6/       pp-ocrv6_small_det.onnx, pp-ocrv6_small_rec.onnx, ppocrv6_dict.txt
//! Nothing is downloaded. Without the OCR files, text layers are still read and pages that are
//! only an image are reported as empty, so a missing model never passes for an empty scan.

use crate::app::{App, WriteOutcome, WriteRequest};
use crate::cli::DocumentsCommand;
use crate::import::slug::slugify;
use cyberbrain_core::source::Document;
use cyberbrain_core::types::{NoteKind, Ring};
use cyberbrain_core::{Error, Result, validate_name};
use cyberbrain_documents::{
    ModelSize, PageReport, PageRoute, PdfSource, load_onnx_runtime, model_files, ocr_pipeline,
    onnx_runtime_file,
};
use cyberbrain_policy::OperatorChoice;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// One file's place in the parallel read: not read yet, or read with its file name.
type Slot = Option<Result<(String, cyberbrain_documents::Read)>>;

/// Tag naming the file a document note came from, as in `import`.
const SOURCE_TAG_PREFIX: &str = "src:";
/// Tag with the hash of the original bytes: an unchanged file is recognised without reading it.
const HASH_TAG_PREFIX: &str = "doc-hash:";

#[derive(Debug, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum Outcome {
    Written {
        name: String,
        pages: usize,
        ocr_pages: usize,
        empty_pages: usize,
    },
    Replaced {
        name: String,
        pages: usize,
        ocr_pages: usize,
        empty_pages: usize,
    },
    Unchanged {
        name: String,
    },
    /// A note of that name exists and did not come from this file: left alone.
    Exists {
        name: String,
        why: String,
    },
    /// Held by the PII gate; nothing was written. `--accept-pii` takes it after review.
    Held {
        name: String,
        findings: usize,
    },
    Failed {
        error: String,
    },
}

#[derive(Debug, Serialize)]
pub struct FileReport {
    pub file: String,
    #[serde(flatten)]
    pub outcome: Outcome,
}

#[derive(Debug, Serialize)]
pub struct AddReport {
    pub files: Vec<FileReport>,
    pub ocr: bool,
    pub dry_run: bool,
}

#[derive(Debug, Serialize)]
pub struct Listed {
    pub name: String,
    pub file: String,
    pub pages: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub area: Option<String>,
}

#[derive(Serialize)]
#[serde(untagged)]
pub enum Report {
    Add(AddReport),
    List(Vec<Listed>),
}

/// Runs a `documents` subcommand. Exit code: 0 clean, 1 something failed, 3 only PII holds.
pub fn run(app: &App, command: DocumentsCommand) -> Result<(Report, i32)> {
    match command {
        DocumentsCommand::Add {
            paths,
            area,
            threads,
            accept_pii,
            dry_run,
        } => {
            let r = add(app, &paths, area, threads, accept_pii, dry_run)?;
            let code = if r
                .files
                .iter()
                .any(|f| matches!(f.outcome, Outcome::Failed { .. }))
            {
                1
            } else if r
                .files
                .iter()
                .any(|f| matches!(f.outcome, Outcome::Held { .. }))
            {
                3
            } else {
                0
            };
            Ok((Report::Add(r), code))
        }
        DocumentsCommand::List { area } => Ok((Report::List(list(app, area.as_deref())?), 0)),
    }
}

fn pdfs_under(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack: Vec<PathBuf> = paths.to_vec();
    while let Some(p) = stack.pop() {
        if p.is_dir() {
            let entries = std::fs::read_dir(&p).map_err(|e| Error::Io {
                path: p.clone(),
                source: e,
            })?;
            for e in entries.flatten() {
                stack.push(e.path());
            }
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("pdf")) {
            out.push(p);
        } else if paths.contains(&p) {
            return Err(Error::Config(format!(
                "{} is not a PDF or a folder",
                p.display()
            )));
        }
    }
    out.sort();
    Ok(out)
}

/// A note name from a file name: `doc-` and the slug, cut to fit the name limit.
fn note_name(path: &Path) -> String {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let mut name = format!("doc-{}", slugify(&stem));
    while name.len() > 120 {
        name.pop();
    }
    name.trim_end_matches('-').to_string()
}

fn add(
    app: &App,
    paths: &[PathBuf],
    area: Option<String>,
    threads: Option<usize>,
    accept_pii: bool,
    dry_run: bool,
) -> Result<AddReport> {
    let files = pdfs_under(paths)?;
    let models = app.store().models_dir();
    let threads = threads
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
        })
        .clamp(1, 64);

    // OCR only when everything it needs is there. Each pipeline holds its models in memory
    // (about 0.7 GB with the small models), so threads are also the memory knob.
    let ppocr = models.join("ppocrv6");
    let ort = onnx_runtime_file(&models.join("onnxruntime"));
    let ocr_ready = ort.exists()
        && model_files(&ppocr, ModelSize::Small)
            .iter()
            .all(|p| p.exists());
    let pipelines = if ocr_ready {
        load_onnx_runtime(&ort)?;
        (0..threads)
            .map(|_| ocr_pipeline(&ppocr, ModelSize::Small, 1))
            .collect::<Result<Vec<_>>>()?
    } else {
        Vec::new()
    };
    let source = PdfSource::new(&models.join("pdfium"), pipelines).map_err(|e| {
        Error::Config(format!(
            "{e}. Put the PDFium library for this platform into {}",
            models.join("pdfium").display()
        ))
    })?;

    // Read in parallel; keep the file order for writing and reporting.
    let next = AtomicUsize::new(0);
    let read: Mutex<Vec<Slot>> = Mutex::new((0..files.len()).map(|_| None).collect());
    std::thread::scope(|scope| {
        for _ in 0..threads.min(files.len().max(1)) {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(path) = files.get(i) else { break };
                    let name = path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    let r = std::fs::read(path)
                        .map_err(|e| Error::Io {
                            path: path.clone(),
                            source: e,
                        })
                        .and_then(|bytes| {
                            source.read(&Document {
                                name: &name,
                                media_type: Some("application/pdf"),
                                bytes: &bytes,
                            })
                        })
                        .map(|r| (name, r));
                    read.lock().unwrap_or_else(|p| p.into_inner())[i] = Some(r);
                }
            });
        }
    });

    let mut reports = Vec::with_capacity(files.len());
    let results = read.into_inner().unwrap_or_else(|p| p.into_inner());
    for (path, result) in files.iter().zip(results) {
        let file = path.display().to_string();
        let outcome = match result.expect("every file was read") {
            Err(e) => Outcome::Failed {
                error: e.to_string(),
            },
            Ok((file_name, r)) => {
                write_document(app, path, &file_name, r, area.clone(), accept_pii, dry_run)
                    .unwrap_or_else(|e| Outcome::Failed {
                        error: e.to_string(),
                    })
            }
        };
        reports.push(FileReport { file, outcome });
    }
    Ok(AddReport {
        files: reports,
        ocr: ocr_ready,
        dry_run,
    })
}

fn count(pages: &[PageReport], route: PageRoute) -> usize {
    pages.iter().filter(|p| p.route == route).count()
}

fn write_document(
    app: &App,
    path: &Path,
    file_name: &str,
    read: cyberbrain_documents::Read,
    area: Option<String>,
    accept_pii: bool,
    dry_run: bool,
) -> Result<Outcome> {
    let name = note_name(path);
    validate_name(&name)
        .map_err(|why| Error::Config(format!("no valid note name from {file_name}: {why}")))?;
    let src_tag = format!("{SOURCE_TAG_PREFIX}{file_name}");
    let hash_tag = format!("{HASH_TAG_PREFIX}{}", &read.extracted.source_hash[..16]);
    let mut body = String::new();
    if let Some(t) = &read.extracted.title {
        body.push_str(&format!("# {t}\n\n"));
    }
    body.push_str(&read.extracted.body);
    let (pages, ocr_pages, empty_pages) = (
        read.pages.len(),
        count(&read.pages, PageRoute::Ocr),
        count(&read.pages, PageRoute::Empty),
    );

    let mut replacing = false;
    match app.store().read(&name) {
        Ok(existing) => {
            if !existing.front.tags.contains(&src_tag) {
                return Ok(Outcome::Exists {
                    name,
                    why: "a note of this name exists and was not added from this file".into(),
                });
            }
            if existing.front.tags.contains(&hash_tag)
                && (area.is_none() || existing.front.bereich == area)
            {
                return Ok(Outcome::Unchanged { name });
            }
            replacing = true;
        }
        Err(Error::NoSuchNote(_)) => {}
        Err(e) => return Err(e),
    }
    let req = WriteRequest {
        ring: Ring::External,
        kind: NoteKind::Reference,
        name: name.clone(),
        body,
        tags: vec![src_tag, hash_tag],
        bereich: area.map(Some),
        retention: None,
        supersedes: None,
        valid_from: None,
        invalid_at: None,
        force: false,
        choice: accept_pii.then_some(OperatorChoice::MarkReviewed),
        expected_updated: None,
        arriving: None,
        dry_run,
    };
    Ok(match app.write(req)? {
        WriteOutcome::Written(_) if replacing => Outcome::Replaced {
            name,
            pages,
            ocr_pages,
            empty_pages,
        },
        WriteOutcome::Written(_) => Outcome::Written {
            name,
            pages,
            ocr_pages,
            empty_pages,
        },
        WriteOutcome::Held { findings, .. } => Outcome::Held {
            name,
            findings: findings.len(),
        },
        WriteOutcome::Conflict { .. } => Outcome::Failed {
            error: "the note changed while it was being written".into(),
        },
        WriteOutcome::Quarantined(r) => Outcome::Failed {
            error: format!("quarantined at {}", r.path.display()),
        },
    })
}

fn list(app: &App, area: Option<&str>) -> Result<Vec<Listed>> {
    let store = app.store();
    let mut out = Vec::new();
    for e in store.list()?.entries {
        let Ok(n) = store.read_path(&e.path) else {
            continue;
        };
        let Some(file) = n
            .front
            .tags
            .iter()
            .find_map(|t| t.strip_prefix(SOURCE_TAG_PREFIX))
        else {
            continue;
        };
        if !n.front.tags.iter().any(|t| t.starts_with(HASH_TAG_PREFIX)) {
            continue; // `import` uses `src:` too; only documents carry the hash tag
        }
        if area.is_some() && n.front.bereich.as_deref() != area {
            continue;
        }
        let pages = n.body.matches("<!-- cyberbrain:locator p. ").count();
        out.push(Listed {
            name: n.front.name.clone(),
            file: file.to_string(),
            pages,
            area: n.front.bereich.clone(),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

pub fn render(report: &Report) -> String {
    match report {
        Report::List(items) if items.is_empty() => "no documents in this store\n".into(),
        Report::List(items) => {
            let mut s = String::new();
            for d in items {
                let area = d
                    .area
                    .as_deref()
                    .map(|a| format!("  [{a}]"))
                    .unwrap_or_default();
                s.push_str(&format!("{}  {} p.  {}{}\n", d.name, d.pages, d.file, area));
            }
            s.push_str(&format!(
                "\n{} documents. Remove one with `cyberbrain forget <name>`.\n",
                items.len()
            ));
            s
        }
        Report::Add(r) => {
            let mut s = String::new();
            for f in &r.files {
                let line = match &f.outcome {
                    Outcome::Written {
                        name,
                        pages,
                        ocr_pages,
                        empty_pages,
                    } => {
                        format!(
                            "added     {name} ({pages} p., {ocr_pages} recognised, {empty_pages} empty)"
                        )
                    }
                    Outcome::Replaced {
                        name,
                        pages,
                        ocr_pages,
                        empty_pages,
                    } => {
                        format!(
                            "replaced  {name} ({pages} p., {ocr_pages} recognised, {empty_pages} empty)"
                        )
                    }
                    Outcome::Unchanged { name } => format!("unchanged {name}"),
                    Outcome::Exists { name, why } => format!("skipped   {name}: {why}"),
                    Outcome::Held { name, findings } => {
                        format!(
                            "held      {name}: {findings} possible personal data; review, then --accept-pii"
                        )
                    }
                    Outcome::Failed { error } => format!("failed    {}: {error}", f.file),
                };
                s.push_str(&line);
                s.push('\n');
            }
            if !r.ocr {
                s.push_str(
                    "\nOCR is off: put ONNX Runtime into models/onnxruntime and the PP-OCRv6 files into \
                     models/ppocrv6 to read pages that are only an image.\n",
                );
            }
            if r.dry_run {
                s.push_str("\ndry run: nothing was written\n");
            }
            s
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_names_are_valid_slugs_within_the_limit() {
        assert_eq!(
            note_name(Path::new("/a/Schriftsatz Müller.pdf")),
            "doc-schriftsatz-mueller"
        );
        let long = note_name(Path::new(&format!("{}.pdf", "x".repeat(300))));
        assert!(long.len() <= 120 && validate_name(&long).is_ok(), "{long}");
    }

    #[test]
    fn folders_are_walked_and_a_named_non_pdf_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("akte")).unwrap();
        std::fs::write(dir.path().join("akte/b.PDF"), b"%PDF").unwrap();
        std::fs::write(dir.path().join("a.pdf"), b"%PDF").unwrap();
        std::fs::write(dir.path().join("notiz.txt"), b"x").unwrap();
        let found = pdfs_under(&[dir.path().to_path_buf()]).unwrap();
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(pdfs_under(&[dir.path().join("notiz.txt")]).is_err());
    }
}
