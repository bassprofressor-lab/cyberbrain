//! `cyberbrain documents`: PDFs and scans in, one note per document (feature `documents`).
//!
//! Reading is parallel, writing is not: every note goes through `App::write`, which is where
//! the PII hold, the audit row and the index update live. A document is not a second way into
//! the store with its own rules; it is a note whose body happens to come from a PDF.
//!
//! The two native libraries come from a folder of the user's own, never from the store
//! ([`library_root`]):
//!   <lib>/pdfium/         libpdfium.so | libpdfium.dylib | pdfium.dll
//!   <lib>/onnxruntime/    libonnxruntime.so | libonnxruntime.dylib | onnxruntime.dll
//! The OCR models, which are data, stay in the store next to model2vec:
//!   models/ppocrv6/       pp-ocrv6_small_det.onnx, pp-ocrv6_small_rec.onnx, ppocrv6_dict.txt
//! Nothing is downloaded. Without the OCR files, text layers are still read and pages that are
//! only an image are reported as empty, so a missing model never passes for an empty scan.
//!
//! Until 2026-11-10 the libraries were read from `<store>/models/` as well. A store travels
//! with its repository, so a cloned repository could bring a library there, and the first
//! `documents add` loaded it: its code ran as whoever ran the command (H1 of the review of
//! that day). A library left there now stops the command with what to do instead.

use crate::app::{App, WriteOutcome, WriteRequest};
use crate::cli::DocumentsCommand;
use crate::import::slug::slugify;
use cyberbrain_core::source::Document;
use cyberbrain_core::types::{NoteKind, Ring};
use cyberbrain_core::{Error, Result, validate_name};
use cyberbrain_documents::{
    ModelSize, PageReport, PageRoute, PdfSource, load_onnx_runtime, model_files, ocr_pipeline,
    onnx_runtime_file, pdfium_library_file,
};
use cyberbrain_policy::OperatorChoice;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

/// One file's place in the parallel read: not read yet, or read.
type Slot = Option<Result<cyberbrain_documents::Read>>;

/// Tag naming the file a document note came from, as in `import`.
const SOURCE_TAG_PREFIX: &str = "src:";
/// Tag with the hash of the original bytes: an unchanged file is recognised without reading it.
const HASH_TAG_PREFIX: &str = "doc-hash:";
/// Tag with the hash of where the file lies ([`Origin::key`]): which document a note is.
const ID_TAG_PREFIX: &str = "doc-id:";

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
    /// Things the operator should know that stopped nothing, such as a library still lying
    /// in the store.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
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
    note_name_within(path, 120)
}

fn note_name_within(path: &Path, limit: usize) -> String {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let mut name = format!("doc-{}", slugify(&stem));
    while name.len() > limit {
        name.pop();
    }
    name.trim_end_matches('-').to_string()
}

// ---------------------------------------------------------------------------------------
// Native libraries (H1, 2026-11-10)

/// Names a folder of the operator's choosing for the two libraries. Read from the environment
/// of the person running the command, never from the store or its configuration: both travel
/// with a repository.
pub const LIBRARY_ENV: &str = "CYBERBRAIN_DOCUMENTS_LIB";

/// The folder the libraries are taken from: [`LIBRARY_ENV`] when set, else the user's data
/// folder: `$XDG_DATA_HOME/cyberbrain/lib` (`~/.local/share/cyberbrain/lib`) on Linux,
/// `~/Library/Application Support/cyberbrain/lib` on macOS, `%LOCALAPPDATA%\cyberbrain\lib`
/// on Windows.
pub fn library_root() -> Result<PathBuf> {
    let var = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if let Some(p) = var(LIBRARY_ENV) {
        if !p.is_absolute() {
            return Err(Error::Config(format!(
                "{LIBRARY_ENV} must be an absolute path, not {}",
                p.display()
            )));
        }
        return Ok(p);
    }
    let root = if cfg!(windows) {
        var("LOCALAPPDATA").map(|d| d.join("cyberbrain").join("lib"))
    } else if cfg!(target_os = "macos") {
        var("HOME").map(|h| {
            h.join("Library")
                .join("Application Support")
                .join("cyberbrain")
                .join("lib")
        })
    } else {
        var("XDG_DATA_HOME")
            .filter(|p| p.is_absolute())
            .or_else(|| var("HOME").map(|h| h.join(".local").join("share")))
            .map(|d| d.join("cyberbrain").join("lib"))
    };
    root.ok_or_else(|| {
        Error::Config(format!(
            "no per-user data folder to take the PDFium and ONNX Runtime libraries from \
             (HOME or LOCALAPPDATA is not set); set {LIBRARY_ENV} to a folder of your own"
        ))
    })
}

/// The two libraries, resolved and checked, before anything is loaded.
#[derive(Debug)]
struct Libraries {
    pdfium: PathBuf,
    /// `None`: not installed, recognition stays off.
    onnx_runtime: Option<PathBuf>,
    /// Said in the report: a library still lying in the store, ignored.
    notes: Vec<String>,
}

/// One library: `Some(path)` when it is in the library folder and passes [`trusted_library`];
/// `None` when it is in neither place. A copy in the store's `models/` folder is never
/// loaded: alone it is an error saying what to do, next to a proper one it is a note.
fn locate(
    lib_root: &Path,
    store_root: &Path,
    models: &Path,
    folder: &str,
    file: &Path,
    what: &str,
    notes: &mut Vec<String>,
) -> Result<Option<PathBuf>> {
    let wanted = lib_root.join(folder).join(file);
    let legacy = models.join(folder).join(file);
    let in_store = legacy.symlink_metadata().is_ok();
    if wanted.symlink_metadata().is_ok() {
        if in_store {
            notes.push(format!(
                "{} is ignored: libraries are no longer loaded from the store. Delete it.",
                legacy.display()
            ));
        }
        return trusted_library(&wanted, store_root).map(Some);
    }
    if in_store {
        return Err(Error::Config(format!(
            "{what} was found at {} and was not loaded: cyberbrain no longer loads native \
             libraries from the store. A store travels with its repository, so a library there \
             may have come with a clone, and loading it runs its code as you. If you put it \
             there yourself, move it to {} (or set {LIBRARY_ENV} to a folder of your own), \
             then run the command again.",
            legacy.display(),
            wanted.display()
        )));
    }
    Ok(None)
}

/// Checks a library before it is loaded and returns the path to load: the file with every
/// link resolved, so what was checked is what is loaded. Refused when it resolves into the
/// store (a link from the library folder back into `models/` is the same hole), and on Unix
/// when the file or any folder above it belongs to somebody other than the user or root, or
/// can be written by others: such a library can be swapped by whoever else can write there.
/// A world-writable folder with the sticky bit (`/tmp`) only lets owners replace their own
/// entries and passes, as it does for OpenSSH's `StrictModes`.
fn trusted_library(file: &Path, store_root: &Path) -> Result<PathBuf> {
    let refuse = |why: String| Error::Config(format!("{} was not loaded: {why}", file.display()));
    let real = std::fs::canonicalize(file).map_err(|e| Error::Io {
        path: file.to_path_buf(),
        source: e,
    })?;
    if let Ok(store) = std::fs::canonicalize(store_root)
        && real.starts_with(&store)
    {
        return Err(refuse(format!(
            "it resolves to {}, inside the store; libraries are never loaded from the store",
            real.display()
        )));
    }
    let meta = std::fs::metadata(&real).map_err(|e| Error::Io {
        path: real.clone(),
        source: e,
    })?;
    if !meta.is_file() {
        return Err(refuse("not a regular file".into()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid has no preconditions and cannot fail.
        let me = unsafe { libc::geteuid() };
        let check = |p: &Path, m: &std::fs::Metadata| -> Result<()> {
            if m.uid() != me && m.uid() != 0 {
                return Err(refuse(format!(
                    "{} belongs to user {}, neither you nor root, who could replace it \
                     (an archive unpacked as root keeps its owner: `chown` it, or unpack with \
                     `tar --no-same-owner`)",
                    p.display(),
                    m.uid()
                )));
            }
            let sticky = m.is_dir() && m.mode() & 0o1000 != 0;
            if m.mode() & 0o022 != 0 && !sticky {
                return Err(refuse(format!(
                    "{} can be written by others (mode {:o})",
                    p.display(),
                    m.mode() & 0o7777
                )));
            }
            Ok(())
        };
        check(&real, &meta)?;
        for dir in real.ancestors().skip(1) {
            let m = std::fs::metadata(dir).map_err(|e| Error::Io {
                path: dir.to_path_buf(),
                source: e,
            })?;
            check(dir, &m)?;
        }
    }
    Ok(real)
}

/// Finds both libraries. Nothing is loaded here.
fn libraries(lib_root: &Path, store_root: &Path, models: &Path) -> Result<Libraries> {
    let mut notes = Vec::new();
    let pdfium_file = pdfium_library_file(Path::new(""));
    let pdfium = locate(
        lib_root,
        store_root,
        models,
        "pdfium",
        &pdfium_file,
        "PDFium",
        &mut notes,
    )?
    .ok_or_else(|| {
        Error::Config(format!(
            "PDFium not found. Put {} for this platform into {}",
            pdfium_file.display(),
            lib_root.join("pdfium").display()
        ))
    })?;
    let ort_file = onnx_runtime_file(Path::new(""));
    let onnx_runtime = locate(
        lib_root,
        store_root,
        models,
        "onnxruntime",
        &ort_file,
        "ONNX Runtime",
        &mut notes,
    )?;
    Ok(Libraries {
        pdfium,
        onnx_runtime,
        notes,
    })
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

    // Both libraries are found and checked before either is loaded.
    let libs = libraries(&library_root()?, app.store().root(), &models)?;

    // OCR only when everything it needs is there. Each pipeline holds its models in memory
    // (about 0.7 GB with the small models), so threads are also the memory knob.
    let ppocr = models.join("ppocrv6");
    let ocr_ready = libs.onnx_runtime.is_some()
        && model_files(&ppocr, ModelSize::Small)
            .iter()
            .all(|p| p.exists());
    let pipelines = match &libs.onnx_runtime {
        Some(ort) if ocr_ready => {
            load_onnx_runtime(ort)?;
            (0..threads)
                .map(|_| ocr_pipeline(&ppocr, ModelSize::Small, 1))
                .collect::<Result<Vec<_>>>()?
        }
        _ => Vec::new(),
    };
    let source = PdfSource::with_library(&libs.pdfium, pipelines)?;

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
                        });
                    read.lock().unwrap_or_else(|p| p.into_inner())[i] = Some(r);
                }
            });
        }
    });

    let results = read.into_inner().unwrap_or_else(|p| p.into_inner());
    let items: Vec<(Origin, Result<cyberbrain_documents::Read>)> = files
        .iter()
        .zip(results)
        .map(|(path, r)| (Origin::of(app, path), r.expect("every file was read")))
        .collect();
    let outcomes = write_all(app, items, area, accept_pii, dry_run)?;
    let reports = files
        .iter()
        .zip(outcomes)
        .map(|(path, outcome)| FileReport {
            file: path.display().to_string(),
            outcome,
        })
        .collect();
    Ok(AddReport {
        files: reports,
        ocr: ocr_ready,
        dry_run,
        notes: libs.notes,
    })
}

fn count(pages: &[PageReport], route: PageRoute) -> usize {
    pages.iter().filter(|p| p.route == route).count()
}

// ---------------------------------------------------------------------------------------
// Which note a file belongs to (H3, 2026-11-10)

/// Where a file comes from, as far as its note is concerned.
///
/// The note used to be found by its name alone, `doc-<file stem>`, and the `src:` tag held
/// the bare file name, so `akte-1/urteil.pdf` and `akte-2/urteil.pdf` were one document: the
/// second replaced the first and the next run swapped them back. A document is now known by
/// the place of its file ([`Origin::key`]), carried as a hash in the `doc-id:` tag; the name
/// is only a readable label, with a suffix when the plain one is taken.
#[derive(Debug, Clone)]
struct Origin {
    path: PathBuf,
    file_name: String,
    /// The file's path relative to the project the store belongs to (the store's parent
    /// folder), with `/` between the parts, when it lies inside that project: the same on
    /// every machine that has the repository and for every way of naming the folder on the
    /// command line. A file elsewhere is known by its absolute path, links resolved.
    key: String,
}

impl Origin {
    fn of(app: &App, path: &Path) -> Origin {
        let real = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let project = app
            .store()
            .root()
            .parent()
            .and_then(|p| std::fs::canonicalize(p).ok());
        let key = match project.as_deref().and_then(|p| real.strip_prefix(p).ok()) {
            Some(rel) => rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/"),
            None => cyberbrain_core::slash(&real),
        };
        Origin {
            path: path.to_path_buf(),
            file_name: path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            key,
        }
    }

    /// `doc-id:` and 16 hex digits of the key's hash. A hash and not the path: folder names
    /// carry client names and file numbers, and a tag is indexed and shown unredacted.
    fn id_tag(&self) -> String {
        let h = cyberbrain_core::source::source_hash(self.key.as_bytes());
        format!("{ID_TAG_PREFIX}{}", &h[..16])
    }

    fn src_tag(&self) -> String {
        format!("{SOURCE_TAG_PREFIX}{}", self.file_name)
    }
}

/// The document notes already in the store, by identity.
#[derive(Debug, Default)]
struct Known {
    /// `doc-id:` tag -> note name.
    by_id: HashMap<String, String>,
    /// Notes written before `doc-id:` existed: `src:` tag -> (note name, `doc-hash:` tag).
    legacy: HashMap<String, Vec<(String, String)>>,
}

impl Known {
    fn scan(app: &App) -> Result<Known> {
        let store = app.store();
        let mut known = Known::default();
        for e in store.list()?.entries {
            let Ok(n) = store.read_path(&e.path) else {
                continue;
            };
            let tags = &n.front.tags;
            let Some(hash) = tags.iter().find(|t| t.starts_with(HASH_TAG_PREFIX)) else {
                continue; // not a document
            };
            if let Some(id) = tags.iter().find(|t| t.starts_with(ID_TAG_PREFIX)) {
                known.by_id.insert(id.clone(), n.front.name.clone());
            } else if let Some(src) = tags.iter().find(|t| t.starts_with(SOURCE_TAG_PREFIX)) {
                known
                    .legacy
                    .entry(src.clone())
                    .or_default()
                    .push((n.front.name.clone(), hash.clone()));
            }
        }
        Ok(known)
    }
}

/// Writes every read document to its note, in order. A note from before `doc-id:` existed
/// is taken over by one file of the same file name, the one whose bytes it holds when there
/// is one, so an old store is neither doubled nor left with an orphan.
fn write_all(
    app: &App,
    items: Vec<(Origin, Result<cyberbrain_documents::Read>)>,
    area: Option<String>,
    accept_pii: bool,
    dry_run: bool,
) -> Result<Vec<Outcome>> {
    let mut known = Known::scan(app)?;
    let mut claims: HashMap<usize, String> = HashMap::new();
    for (src, notes) in &known.legacy {
        for (name, hash) in notes {
            let candidates: Vec<usize> = items
                .iter()
                .enumerate()
                .filter(|(i, (o, _))| {
                    o.src_tag() == *src
                        && !known.by_id.contains_key(&o.id_tag())
                        && !claims.contains_key(i)
                })
                .map(|(i, _)| i)
                .collect();
            let same_bytes = candidates
                .iter()
                .copied()
                .find(|i| matches!(&items[*i].1, Ok(r) if hash_tag(r) == *hash));
            if let Some(i) = same_bytes.or(candidates.first().copied()) {
                claims.insert(i, name.clone());
            }
        }
    }
    let mut taken: HashSet<String> = HashSet::new();
    let mut out = Vec::with_capacity(items.len());
    for (i, (origin, read)) in items.into_iter().enumerate() {
        let outcome = match read {
            Err(e) => Outcome::Failed {
                error: e.to_string(),
            },
            Ok(r) => write_document(
                app,
                &mut known,
                &mut taken,
                claims.get(&i).cloned(),
                &origin,
                r,
                area.clone(),
                accept_pii,
                dry_run,
            )
            .unwrap_or_else(|e| Outcome::Failed {
                error: e.to_string(),
            }),
        };
        out.push(outcome);
    }
    Ok(out)
}

fn hash_tag(read: &cyberbrain_documents::Read) -> String {
    format!("{HASH_TAG_PREFIX}{}", &read.extracted.source_hash[..16])
}

/// A free name for a new document note: `doc-<stem>`, or with the first six digits of its
/// identity when that is taken by another note or by another file in this run.
fn free_name(app: &App, taken: &HashSet<String>, origin: &Origin) -> Result<Option<String>> {
    let id = origin.id_tag();
    let suffix = &id[ID_TAG_PREFIX.len()..ID_TAG_PREFIX.len() + 6];
    for name in [
        note_name(&origin.path),
        format!("{}-{suffix}", note_name_within(&origin.path, 120 - 7)),
    ] {
        validate_name(&name).map_err(|why| {
            Error::Config(format!(
                "no valid note name from {}: {why}",
                origin.file_name
            ))
        })?;
        if taken.contains(&name) {
            continue;
        }
        match app.store().read(&name) {
            Err(Error::NoSuchNote(_)) => return Ok(Some(name)),
            Ok(_) => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(None)
}

#[allow(clippy::too_many_arguments)]
fn write_document(
    app: &App,
    known: &mut Known,
    taken: &mut HashSet<String>,
    claim: Option<String>,
    origin: &Origin,
    read: cyberbrain_documents::Read,
    area: Option<String>,
    accept_pii: bool,
    dry_run: bool,
) -> Result<Outcome> {
    let src_tag = origin.src_tag();
    let id_tag = origin.id_tag();
    let hash_tag = hash_tag(&read);
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

    // This file's note: by identity, else an old note it takes over, else a new one.
    let (name, replacing) = match known.by_id.get(&id_tag).cloned().or(claim) {
        Some(name) => {
            let note = app.store().read(&name)?;
            // Unchanged also for an old note without `doc-id:` that holds these very bytes:
            // rewritten only to add the tag, a note whose personal data a person once marked
            // as reviewed would be held again. It is claimed the same way on every run and
            // gets its tag when its file changes.
            if note.front.tags.contains(&hash_tag) && (area.is_none() || note.front.bereich == area)
            {
                taken.insert(name.clone());
                return Ok(Outcome::Unchanged { name });
            }
            (name, true)
        }
        None => match free_name(app, taken, origin)? {
            Some(name) => (name, false),
            None => {
                return Ok(Outcome::Exists {
                    name: note_name(&origin.path),
                    why: "this name and the one with the file's suffix are both taken by other \
                          notes"
                        .into(),
                });
            }
        },
    };
    taken.insert(name.clone());
    let req = WriteRequest {
        ring: Ring::External,
        kind: NoteKind::Reference,
        name: name.clone(),
        body,
        tags: vec![src_tag, hash_tag, id_tag.clone()],
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
    let outcome = match app.write(req)? {
        WriteOutcome::Written(_) if replacing => Outcome::Replaced {
            name: name.clone(),
            pages,
            ocr_pages,
            empty_pages,
        },
        WriteOutcome::Written(_) => Outcome::Written {
            name: name.clone(),
            pages,
            ocr_pages,
            empty_pages,
        },
        WriteOutcome::Held { findings, .. } => {
            return Ok(Outcome::Held {
                name,
                findings: findings.len(),
            });
        }
        WriteOutcome::Conflict { .. } => {
            return Ok(Outcome::Failed {
                error: "the note changed while it was being written".into(),
            });
        }
        WriteOutcome::Quarantined(r) => {
            return Ok(Outcome::Failed {
                error: format!("quarantined at {}", r.path.display()),
            });
        }
    };
    if !dry_run {
        known.by_id.insert(id_tag, name);
    }
    Ok(outcome)
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
                    "\nOCR is off: put ONNX Runtime into the library folder's onnxruntime/ (see \
                     docs/DOCUMENTS.md) and the PP-OCRv6 files into the store's models/ppocrv6 to \
                     read pages that are only an image.\n",
                );
            }
            for n in &r.notes {
                s.push_str(&format!("\nnote: {n}\n"));
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

    // ---------------------------------------------------------------------------------
    // H1 and H3 (review 2026-11-10)

    use crate::app::RecallRequest;
    use cyberbrain_policy::Actor;

    fn store() -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("store");
        App::init(&root, &Actor::Operator).unwrap();
        let app = App::open(Some(&root), Actor::Operator).unwrap();
        (dir, app)
    }

    /// Writes `files` (path under `root`, text) to disk and through the write path, as `add`
    /// does after reading them, without PDFium: the body is the text, the hash that of the
    /// bytes.
    fn ingest(app: &App, root: &Path, files: &[(&str, &str)], area: Option<&str>) -> Vec<Outcome> {
        let mut items = Vec::new();
        for (rel, text) in files {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
            let read = cyberbrain_documents::Read {
                extracted: cyberbrain_core::source::Extracted {
                    title: None,
                    body: text.to_string(),
                    source_hash: cyberbrain_core::source::source_hash(text.as_bytes()),
                },
                pages: Vec::new(),
            };
            items.push((Origin::of(app, &path), Ok(read)));
        }
        write_all(app, items, area.map(str::to_string), false, false).unwrap()
    }

    fn documents(app: &App) -> Vec<(String, String)> {
        list(app, None)
            .unwrap()
            .into_iter()
            .map(|d| {
                let body = app.store().read(&d.name).unwrap().body;
                (d.name, body)
            })
            .collect()
    }

    fn found(app: &App, q: &str) -> Vec<String> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(app.recall(q, &RecallRequest::default()))
            .unwrap()
            .hits
            .into_iter()
            .map(|h| h.note_name)
            .collect()
    }

    fn kind(o: &Outcome) -> &'static str {
        match o {
            Outcome::Written { .. } => "written",
            Outcome::Replaced { .. } => "replaced",
            Outcome::Unchanged { .. } => "unchanged",
            Outcome::Exists { .. } => "exists",
            Outcome::Held { .. } => "held",
            Outcome::Failed { .. } => "failed",
        }
    }

    const EINS: &str = "Kuendigungsschutzklage abgewiesen, Revision nicht zugelassen.";
    const ZWEI: &str = "Mietminderung wegen Schimmel im Bad um zwanzig Prozent.";

    /// H3: `akte-1/urteil.pdf` and `akte-2/urteil.pdf` were both `doc-urteil`; the second
    /// replaced the first, and the next run swapped them back.
    #[test]
    fn two_files_of_one_name_in_two_folders_are_two_notes() {
        let (dir, app) = store();
        let root = dir.path().join("akten");
        let files = [("akte-1/urteil.pdf", EINS), ("akte-2/urteil.pdf", ZWEI)];
        let first = ingest(&app, &root, &files, None);
        assert_eq!(
            first.iter().map(kind).collect::<Vec<_>>(),
            ["written", "written"],
            "{first:?}"
        );
        let docs = documents(&app);
        assert_eq!(docs.len(), 2, "{docs:?}");
        assert!(
            docs.iter()
                .any(|(_, b)| b.contains("Kuendigungsschutzklage"))
        );
        assert!(docs.iter().any(|(_, b)| b.contains("Mietminderung")));
        let a = found(&app, "Kuendigungsschutzklage");
        let b = found(&app, "Mietminderung");
        assert!(
            !a.is_empty() && !b.is_empty() && a[0] != b[0],
            "{a:?} {b:?}"
        );

        // A second run changes nothing, in either order.
        let again = ingest(&app, &root, &files, None);
        assert_eq!(
            again.iter().map(kind).collect::<Vec<_>>(),
            ["unchanged", "unchanged"],
            "{again:?}"
        );
        let mut reversed = files;
        reversed.reverse();
        let again = ingest(&app, &root, &reversed, None);
        assert_eq!(
            again.iter().map(kind).collect::<Vec<_>>(),
            ["unchanged", "unchanged"],
            "{again:?}"
        );
        assert_eq!(documents(&app), docs);
        // With an area each keeps its own note too: nothing moves between them.
        let area = ingest(&app, &root, &files, Some("matter-1"));
        assert_eq!(
            area.iter().map(kind).collect::<Vec<_>>(),
            ["replaced", "replaced"]
        );
        assert_eq!(documents(&app).len(), 2);
    }

    /// A new version of the same file still replaces its note.
    #[test]
    fn a_new_version_of_the_same_file_replaces_its_note() {
        let (dir, app) = store();
        let root = dir.path().join("akten");
        let one = ingest(&app, &root, &[("akte-1/urteil.pdf", EINS)], None);
        let two = ingest(&app, &root, &[("akte-1/urteil.pdf", ZWEI)], None);
        assert_eq!(kind(&one[0]), "written");
        assert_eq!(kind(&two[0]), "replaced", "{two:?}");
        let docs = documents(&app);
        assert_eq!(docs.len(), 1, "{docs:?}");
        assert!(docs[0].1.contains("Mietminderung"));
    }

    /// A store written before the fix holds `doc-<stem>` tagged with the bare file name. Its
    /// notes are taken over, not doubled and not left behind.
    #[test]
    fn notes_from_before_the_fix_are_taken_over_not_doubled() {
        let (dir, app) = store();
        let legacy = |name: &str, file: &str, text: &str| {
            let hash = cyberbrain_core::source::source_hash(text.as_bytes());
            app.write(WriteRequest {
                ring: Ring::External,
                kind: NoteKind::Reference,
                name: name.into(),
                body: text.into(),
                tags: vec![format!("src:{file}"), format!("doc-hash:{}", &hash[..16])],
                bereich: None,
                retention: None,
                supersedes: None,
                valid_from: None,
                invalid_at: None,
                force: false,
                choice: None,
                expected_updated: None,
                arriving: None,
                dry_run: false,
            })
            .unwrap();
        };
        // The last run before the fix left akte-1's text under `doc-urteil`, and a flat
        // folder left `doc-beschluss`.
        legacy("doc-urteil", "urteil.pdf", EINS);
        legacy(
            "doc-beschluss",
            "beschluss.pdf",
            "Beschluss ueber die Kosten.",
        );

        let root = dir.path().join("akten");
        let flat = ingest(
            &app,
            &root,
            &[("beschluss.pdf", "Beschluss ueber die Kosten.")],
            None,
        );
        assert_eq!(kind(&flat[0]), "unchanged", "{flat:?}");
        let files = [("akte-1/urteil.pdf", EINS), ("akte-2/urteil.pdf", ZWEI)];
        ingest(&app, &root, &files, None);
        let docs = documents(&app);
        assert_eq!(docs.len(), 3, "{docs:?}");
        assert_eq!(
            docs.iter()
                .filter(|(_, b)| b.contains("Kuendigungsschutzklage"))
                .count(),
            1,
            "{docs:?}"
        );
        assert!(docs.iter().any(|(_, b)| b.contains("Mietminderung")));
        let again = ingest(&app, &root, &files, None);
        assert_eq!(
            again.iter().map(kind).collect::<Vec<_>>(),
            ["unchanged", "unchanged"],
            "{again:?}"
        );
        assert_eq!(documents(&app).len(), 3);
    }

    /// The checks that run before anything is loaded, without loading anything.
    #[cfg(unix)]
    #[test]
    fn libraries_come_from_the_library_folder_and_only_when_nobody_else_can_swap_them() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, app) = store();
        let store_root = app.store().root().to_path_buf();
        let models = app.store().models_dir();
        let lib = dir.path().join("lib");
        let pdfium_name = pdfium_library_file(Path::new(""));
        let place = |root: &Path| {
            let f = root.join("pdfium").join(&pdfium_name);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(&f, b"not really a library").unwrap();
            f
        };

        // Nowhere: says where it goes.
        let e = libraries(&lib, &store_root, &models)
            .unwrap_err()
            .to_string();
        assert!(e.contains("PDFium not found") && e.contains("lib"), "{e}");

        // Only in the store: refused, with what to do.
        let legacy = place(&models);
        let e = libraries(&lib, &store_root, &models)
            .unwrap_err()
            .to_string();
        assert!(e.contains("no longer") && e.contains("move it"), "{e}");

        // In the library folder: taken, and the store's copy is named as ignored.
        let proper = place(&lib);
        let found = libraries(&lib, &store_root, &models).unwrap();
        assert_eq!(found.pdfium, std::fs::canonicalize(&proper).unwrap());
        assert!(found.onnx_runtime.is_none());
        assert!(
            found.notes.iter().any(|n| n.contains("ignored")),
            "{found:?}"
        );

        // Writable by others: refused, file or folder.
        std::fs::set_permissions(&proper, std::fs::Permissions::from_mode(0o666)).unwrap();
        let e = libraries(&lib, &store_root, &models)
            .unwrap_err()
            .to_string();
        assert!(e.contains("written by others"), "{e}");
        std::fs::set_permissions(&proper, std::fs::Permissions::from_mode(0o644)).unwrap();
        let folder = proper.parent().unwrap();
        std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o777)).unwrap();
        let e = libraries(&lib, &store_root, &models)
            .unwrap_err()
            .to_string();
        assert!(e.contains("written by others"), "{e}");
        std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o755)).unwrap();

        // A link from the library folder back into the store is the same hole.
        std::fs::remove_file(&proper).unwrap();
        std::os::unix::fs::symlink(&legacy, &proper).unwrap();
        let e = libraries(&lib, &store_root, &models)
            .unwrap_err()
            .to_string();
        assert!(e.contains("inside the store"), "{e}");
    }

    #[test]
    fn the_library_folder_is_named_by_the_operator_or_is_the_users_own() {
        // Only the shape: the environment is shared between tests, so it is not changed here.
        let root = library_root().unwrap();
        assert!(root.is_absolute(), "{}", root.display());
        assert!(
            root.ends_with(Path::new("cyberbrain").join("lib"))
                || std::env::var_os(LIBRARY_ENV).is_some(),
            "{}",
            root.display()
        );
    }

    /// Notes stay valid names when the plain one is taken and the stem is long.
    #[test]
    fn a_taken_name_gets_the_files_suffix_and_stays_valid() {
        let (dir, app) = store();
        let root = dir.path().join("akten");
        let long = format!("{}.pdf", "langer-name-".repeat(15));
        let files = [(format!("a/{long}"), EINS), (format!("b/{long}"), ZWEI)];
        let files: Vec<(&str, &str)> = files.iter().map(|(p, t)| (p.as_str(), *t)).collect();
        let out = ingest(&app, &root, &files, None);
        let names: Vec<String> = documents(&app).into_iter().map(|(n, _)| n).collect();
        assert_eq!(names.len(), 2, "{out:?}");
        for n in &names {
            assert!(validate_name(n).is_ok() && n.len() <= 120, "{n}");
        }
    }

    /// H1: a library in the store's `models/` folder must never be loaded. A repository can
    /// bring that folder, and loading runs its code as whoever ran `documents add`. The stand-in
    /// library only leaves a marker file when loaded; nothing in it is harmful.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_library_in_the_store_is_never_loaded() {
        let (dir, app) = store();
        let marker = dir.path().join("loaded");
        let src = dir.path().join("marker.c");
        std::fs::write(
            &src,
            "#include <stdio.h>\n__attribute__((constructor)) static void m(void) \
             { FILE *f = fopen(MARKER, \"w\"); if (f) fclose(f); }\n",
        )
        .unwrap();
        let lib = dir.path().join("marker.so");
        let built = std::process::Command::new("cc")
            .args(["-shared", "-fPIC", "-o"])
            .arg(&lib)
            .arg(format!("-DMARKER=\"{}\"", marker.display()))
            .arg(&src)
            .status();
        if !built.is_ok_and(|s| s.success()) {
            eprintln!("no C compiler: the marker library could not be built, test skipped");
            return;
        }
        let models = app.store().models_dir();
        for (folder, file) in [
            ("pdfium", "libpdfium.so"),
            ("onnxruntime", "libonnxruntime.so"),
        ] {
            std::fs::create_dir_all(models.join(folder)).unwrap();
            std::fs::copy(&lib, models.join(folder).join(file)).unwrap();
        }
        let ppocr = models.join("ppocrv6");
        std::fs::create_dir_all(&ppocr).unwrap();
        for f in model_files(&ppocr, ModelSize::Small) {
            std::fs::write(f, b"").unwrap();
        }
        let input = dir.path().join("in");
        std::fs::create_dir_all(&input).unwrap();
        std::fs::write(input.join("a.pdf"), b"%PDF-1.4\n").unwrap();
        let r = run(
            &app,
            DocumentsCommand::Add {
                paths: vec![input],
                area: None,
                threads: Some(1),
                accept_pii: false,
                dry_run: true,
            },
        );
        assert!(
            !marker.exists(),
            "a library from the store's models/ folder was loaded"
        );
        let e = r.err().map(|e| e.to_string()).unwrap_or_default();
        assert!(e.contains("no longer"), "{e}");
    }
}
