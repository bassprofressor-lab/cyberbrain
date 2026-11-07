//! Documents into notes: a PDF, with or without a text layer, becomes the body of one note with
//! a locator line (`p. <n>`) before every page, so each block the core cuts from it carries the
//! page it came from and a citation can say where to look.
//!
//! Everything runs in this process on Windows, macOS and Linux alike: PDFium reads the text
//! layer and renders a page image, PP-OCRv6 through ONNX Runtime reads pages that are only an
//! image. Nothing is downloaded at run time; the PDFium library and the model files are looked
//! up in folders the caller names (the CLI uses the store's `models/` folder), which keeps the
//! "no network in the core" rule and lets an air-gapped office install them by copying files.
//!
//! Measured on 1,400 federal court decisions (10,171 pages) and 200 simulated scans, 07.11.2026:
//! see `docs/` in the private evaluation repository for the full table. The numbers that shaped
//! the code are in the comments where they apply.

use cyberbrain_core::source::{BodyBuilder, Document, DocumentSource, Extracted, source_hash};
use cyberbrain_core::{Error, Result};
use oar_ocr::core::config::OrtSessionConfig;
use oar_ocr::oarocr::{OAROCR, OAROCRBuilder, OAROCRResult};
use pdfium_render::prelude::*;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, TryLockError};
use std::time::Instant;

fn failure(reason: impl Into<String>) -> Error {
    // `source::convert` fills in the document name.
    Error::Document {
        name: String::new(),
        reason: reason.into(),
    }
}

/// How a page was read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PageRoute {
    /// The PDF's own text layer.
    TextLayer,
    /// Recognised from the rendered page image.
    Ocr,
    /// No text layer and nothing recognised, or recognition off.
    Empty,
}

/// What happened to one page, for reports and the audit trail.
#[derive(Debug, Clone, Serialize)]
pub struct PageReport {
    pub page: u32,
    pub route: PageRoute,
    /// Visible characters (whitespace not counted).
    pub chars: usize,
    pub millis: u64,
}

/// A read document: what a [`DocumentSource`] returns, plus the per-page report.
#[derive(Debug, Clone)]
pub struct Read {
    pub extracted: Extracted,
    pub pages: Vec<PageReport>,
}

/// PP-OCRv6 model size. `Small` is the default: on the scans it was as accurate as `Medium`
/// (0.18 % against 0.17 % character errors) at a fifth of the time and a third of the memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelSize {
    Tiny,
    Small,
    Medium,
}

impl ModelSize {
    fn name(self) -> &'static str {
        match self {
            Self::Tiny => "tiny",
            Self::Small => "small",
            Self::Medium => "medium",
        }
    }
}

/// The ONNX Runtime library in `dir` under the platform's name. The official Microsoft builds
/// (github.com/microsoft/onnxruntime releases) work as they come; measured with 1.31.1.
pub fn onnx_runtime_file(dir: &Path) -> PathBuf {
    dir.join(if cfg!(windows) {
        "onnxruntime.dll"
    } else if cfg!(target_os = "macos") {
        "libonnxruntime.dylib"
    } else {
        "libonnxruntime.so"
    })
}

/// Loads ONNX Runtime from a library file, once per process, before the first pipeline is
/// built. Loading at run time rather than linking keeps the build free of a download and of
/// OpenSSL, and lets an office put the library next to the models without a toolchain.
pub fn load_onnx_runtime(library: &Path) -> Result<()> {
    static LOADED: OnceLock<std::result::Result<(), String>> = OnceLock::new();
    LOADED
        .get_or_init(|| {
            if !library.exists() {
                return Err(format!("ONNX Runtime not found at {}", library.display()));
            }
            ort::init_from(library)
                .map_err(|e| format!("ONNX Runtime did not load from {}: {e}", library.display()))?
                .commit();
            Ok(())
        })
        .clone()
        .map_err(failure)
}

/// The files PP-OCRv6 needs in `dir`, by size: detection model, recognition model, dictionary.
/// They are the official PaddlePaddle ONNX exports (Hugging Face `PaddlePaddle/PP-OCRv6_*_onnx`).
pub fn model_files(dir: &Path, size: ModelSize) -> [PathBuf; 3] {
    let s = size.name();
    let dict = if size == ModelSize::Tiny {
        "ppocrv6_tiny_dict.txt"
    } else {
        "ppocrv6_dict.txt"
    };
    [
        dir.join(format!("pp-ocrv6_{s}_det.onnx")),
        dir.join(format!("pp-ocrv6_{s}_rec.onnx")),
        dir.join(dict),
    ]
}

/// One PP-OCRv6 pipeline with the official detection thresholds (score 0.2, box 0.45, 0.4 for
/// tiny, unclip 1.4, up to 3,000 candidates), the same values `OAROCRBuilder::pp_ocrv6` sets,
/// but with model files from `dir` instead of names resolved through a download registry.
pub fn ocr_pipeline(dir: &Path, size: ModelSize, threads: usize) -> Result<OAROCR> {
    let [det, rec, dict] = model_files(dir, size);
    for p in [&det, &rec, &dict] {
        if !p.exists() {
            return Err(failure(format!(
                "OCR model file missing: {} (copy the PP-OCRv6 files into {})",
                p.display(),
                dir.display()
            )));
        }
    }
    let detection = oar_ocr::domain::tasks::TextDetectionConfig {
        score_threshold: 0.2,
        box_threshold: if size == ModelSize::Tiny { 0.4 } else { 0.45 },
        unclip_ratio: 1.4,
        max_candidates: 3000,
        ..Default::default()
    };
    OAROCRBuilder::new(det, rec, dict)
        .text_detection_config(detection)
        .ort_session(
            OrtSessionConfig::new()
                .with_intra_threads(threads)
                .with_inter_threads(1),
        )
        .build()
        .map_err(|e| failure(format!("PP-OCRv6 did not load: {e}")))
}

/// Reads PDFs through PDFium and recognises image-only pages with PP-OCRv6.
pub struct PdfSource {
    pdfium: Pdfium,
    /// One pipeline per thread that may read at the same time. An ONNX session runs one request
    /// at a time; with a single shared pipeline twelve threads read scans exactly as fast as
    /// four (0.3 pages/s, measured). With one each: 2.3 pages/s on 12 threads.
    ocr: Vec<Mutex<OAROCR>>,
    /// Resolution of the page image handed to recognition. PP-OCRv6 reads best at 100–150 dpi:
    /// at 300 dpi it dropped the space between words at 7.9 % of word boundaries ("dasProdukt"),
    /// at 120 dpi at 0.03 %, with character errors falling from 1.28 % to 0.24 %. 8 pt print
    /// read as well at 120 as at 150 dpi.
    pub dpi: f32,
    /// A page with fewer visible characters than this AND an image on it is recognised. A short
    /// text page without an image (the judges' names on a last page) is kept as it is.
    pub min_chars: usize,
}

impl PdfSource {
    /// `library_dir`: folder holding `pdfium.dll`, `libpdfium.dylib` or `libpdfium.so`.
    /// `ocr`: pipelines from [`ocr_pipeline`], one per thread that will call [`Self::read`];
    /// empty turns recognition off (pages without a text layer then come back empty).
    pub fn new(library_dir: &Path, ocr: Vec<OAROCR>) -> Result<Self> {
        let bindings =
            Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(library_dir))
                .map_err(|e| {
                    failure(format!(
                        "PDFium not found in {}: {e}",
                        library_dir.display()
                    ))
                })?;
        Ok(Self {
            pdfium: Pdfium::new(bindings),
            ocr: ocr.into_iter().map(Mutex::new).collect(),
            dpi: 120.0,
            min_chars: 25,
        })
    }

    fn recognise(&self, page: &PdfPage<'_>) -> Result<String> {
        if self.ocr.is_empty() {
            return Ok(String::new());
        }
        let image = page
            .render_with_config(&PdfRenderConfig::new().scale_page_by_factor(self.dpi / 72.0))
            .and_then(|b| b.as_image())
            .map_err(|e| failure(format!("page image: {e}")))?
            .to_rgb8();
        // The first free pipeline; when all are busy, wait briefly and look again.
        let result = loop {
            if let Some(ocr) = self.ocr.iter().find_map(|m| match m.try_lock() {
                Ok(g) => Some(g),
                Err(TryLockError::Poisoned(p)) => Some(p.into_inner()),
                Err(TryLockError::WouldBlock) => None,
            }) {
                break ocr
                    .predict(vec![image])
                    .map_err(|e| failure(format!("recognition: {e}")))?;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        Ok(result.first().map(ocr_text).unwrap_or_default())
    }

    /// Reads one PDF and reports, page by page, which route was taken.
    pub fn read(&self, doc: &Document<'_>) -> Result<Read> {
        let pdf = self
            .pdfium
            .load_pdf_from_byte_slice(doc.bytes, None)
            .map_err(|e| failure(format!("not a readable PDF: {e}")))?;
        let title_meta = pdf
            .metadata()
            .get(PdfDocumentMetadataTagType::Title)
            .map(|t| sanitize(t.value()).trim().to_string())
            .filter(|t| !t.is_empty());
        let visible = |s: &str| s.chars().filter(|c| !c.is_whitespace()).count();
        let mut body = BodyBuilder::new();
        let mut pages = Vec::new();
        let mut first_line = None;
        for (i, page) in pdf.pages().iter().enumerate() {
            let number = i as u32 + 1;
            let t0 = Instant::now();
            let mut text = clean_page(&page.text().map(|t| text_layer(&t)).unwrap_or_default());
            let mut route = PageRoute::TextLayer;
            if visible(&text) < self.min_chars
                && page
                    .objects()
                    .iter()
                    .any(|o| o.object_type() == PdfPageObjectType::Image)
            {
                text = if self.ocr.is_empty() {
                    String::new()
                } else {
                    clean_page(&self.recognise(&page)?)
                };
                route = if visible(&text) > 0 {
                    PageRoute::Ocr
                } else {
                    PageRoute::Empty
                };
            } else if visible(&text) == 0 {
                route = PageRoute::Empty;
            }
            if first_line.is_none() {
                first_line = text
                    .lines()
                    .map(str::trim)
                    .find(|l| !l.is_empty())
                    .map(|l| l.chars().take(160).collect::<String>());
            }
            pages.push(PageReport {
                page: number,
                route,
                chars: visible(&text),
                millis: t0.elapsed().as_millis() as u64,
            });
            let locator = format!("p. {number}");
            body.part(
                Some(&locator),
                if text.is_empty() {
                    "(no readable text on this page)"
                } else {
                    &text
                },
            )?;
        }
        Ok(Read {
            extracted: Extracted {
                title: title_meta.or(first_line),
                body: body.finish(),
                source_hash: source_hash(doc.bytes),
            },
            pages,
        })
    }
}

impl DocumentSource for PdfSource {
    fn id(&self) -> &str {
        "pdf"
    }

    fn accepts(&self, doc: &Document<'_>) -> bool {
        matches!(doc.media_type, Some("application/pdf"))
            || doc.name.to_ascii_lowercase().ends_with(".pdf")
    }

    fn extract(&self, doc: &Document<'_>) -> Result<Extracted> {
        Ok(self.read(doc)?.extracted)
    }
}

/// Control characters (except newline and tab) and invisible format characters removed. A NUL
/// at the end of a PDF title (ImageMagick writes them) made notes binary to `grep`; a soft
/// hyphen (U+00AD) splits a word for search.
pub fn sanitize(s: &str) -> String {
    s.chars()
        .filter(|&c| {
            c == '\n'
                || c == '\t'
                || !(c.is_control()
                    || matches!(
                        c,
                        '\u{ad}' | '\u{200b}'..='\u{200f}' | '\u{2060}' | '\u{feff}'
                    ))
        })
        .collect()
}

/// `§133` becomes `§ 133`, as German typesetting has it. Recognition sometimes drops that
/// space, and for search `§133` and `§ 133` are different words.
pub fn space_after_section_sign(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        out.push(c);
        if c == '§' && chars.peek().is_some_and(char::is_ascii_digit) {
            out.push(' ');
        }
    }
    out
}

/// One page of text made fit for a note body: sanitised, a word hyphenated across a line end
/// joined again when the next line starts in lower case, runs of blank lines cut to one, and a
/// line Markdown would read as a heading escaped.
pub fn clean_page(raw: &str) -> String {
    let s = space_after_section_sign(&sanitize(&raw.replace("\r\n", "\n")));
    let lines: Vec<&str> = s.lines().map(str::trim_end).collect();
    let mut joined: Vec<String> = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        let mut line = lines[i].to_string();
        while line.ends_with('-')
            && !line.ends_with(" -")
            && line.chars().rev().nth(1).is_some_and(char::is_alphabetic)
            && i + 1 < lines.len()
            && lines[i + 1]
                .trim_start()
                .chars()
                .next()
                .is_some_and(char::is_lowercase)
        {
            line.pop();
            let next = lines[i + 1].trim_start();
            let (word, rest) = next.split_once(' ').unwrap_or((next, ""));
            line.push_str(word);
            i += 1;
            if !rest.is_empty() {
                joined.push(line);
                line = rest.to_string();
            }
        }
        joined.push(line);
        i += 1;
    }
    let mut text = String::new();
    let mut blank = 0;
    for line in joined {
        if line.trim().is_empty() {
            blank += 1;
            if blank == 1 {
                text.push('\n');
            }
            continue;
        }
        blank = 0;
        if line.trim_start().starts_with('#') {
            text.push('\\');
        }
        text.push_str(&line);
        text.push('\n');
    }
    text.trim_matches('\n').to_string()
}

/// A page's text layer with its paragraphs. `PdfPageText::all()` gives lines without blank
/// lines between paragraphs, and the core cuts blocks at paragraph boundaries: without them
/// blocks grow coarse and citations point less precisely (4 paragraphs instead of 19 in one
/// decision). Lines come from the text segments; a line advance (baseline to baseline) over 1.4
/// times the usual one, or a jump upwards (a new column), starts a paragraph. Measured against
/// Poppler on 100 decisions: same text, 22,109 blocks against 22,150 on the full set.
pub fn text_layer(text: &PdfPageText<'_>) -> String {
    struct Piece {
        left: f32,
        right: f32,
        top: f32,
        bottom: f32,
        text: String,
    }
    struct Line {
        top: f32,
        bottom: f32,
        pieces: Vec<Piece>,
    }
    let mut lines: Vec<Line> = Vec::new();
    for seg in text.segments().iter() {
        let r = seg.bounds();
        let t = seg.text();
        if t.trim().is_empty() {
            continue;
        }
        let p = Piece {
            left: r.left().value,
            right: r.right().value,
            top: r.top().value,
            bottom: r.bottom().value,
            text: t,
        };
        let middle = (p.top + p.bottom) / 2.0;
        match lines.last_mut() {
            Some(l) if middle < l.top && middle > l.bottom => {
                l.top = l.top.max(p.top);
                l.bottom = l.bottom.min(p.bottom);
                l.pieces.push(p);
            }
            _ => lines.push(Line {
                top: p.top,
                bottom: p.bottom,
                pieces: vec![p],
            }),
        }
    }
    for l in &mut lines {
        l.pieces.sort_by(|a, b| a.left.total_cmp(&b.left));
    }
    // Baseline to baseline rather than box height: the boxes hug the glyphs, and measured
    // against them even ordinary line spacing looks like a paragraph break.
    let mut advances: Vec<f32> = lines
        .windows(2)
        .map(|w| w[0].bottom - w[1].bottom)
        .filter(|d| *d > 0.0)
        .collect();
    advances.sort_by(f32::total_cmp);
    let usual = advances.get(advances.len() / 2).copied().unwrap_or(0.0);
    let mut out = String::new();
    let mut previous: Option<&Line> = None;
    for l in &lines {
        if let Some(prev) = previous {
            out.push('\n');
            let advance = prev.bottom - l.bottom; // PDF coordinates: y grows upwards
            if usual > 0.0 && (advance > 1.4 * usual || advance < -0.5 * usual) {
                out.push('\n');
            }
        }
        let mut line = String::new();
        let mut right_before: Option<f32> = None;
        for p in &l.pieces {
            if let Some(rb) = right_before
                && p.left - rb > 0.15 * (l.top - l.bottom)
                && !line.ends_with(' ')
                && !p.text.starts_with(' ')
            {
                line.push(' ');
            }
            line.push_str(&p.text);
            right_before = Some(p.right);
        }
        let mut line = line.trim_end().to_string();
        // PDFium reports a hyphen at a line end as a soft hyphen; keep it as `-` so the word is
        // joined again (sanitize would drop U+00AD and leave two half words).
        if line.ends_with('\u{ad}') || line.ends_with('\u{2}') {
            line.pop();
            line.push('-');
        }
        out.push_str(&line);
        previous = Some(l);
    }
    out
}

/// Recognised regions in reading order: grouped into lines (centre within a line's height),
/// lines top to bottom, regions left to right; a gap over 1.2 times the usual line height
/// becomes a paragraph.
pub fn ocr_text(result: &OAROCRResult) -> String {
    struct Region {
        x: f32,
        y0: f32,
        y1: f32,
        text: String,
    }
    let mut regions: Vec<Region> = result
        .text_regions
        .iter()
        .filter_map(|r| {
            let t = r.text.as_deref()?.trim();
            if t.is_empty() {
                return None;
            }
            let p = &r.bounding_box.points;
            Some(Region {
                x: p.iter().map(|q| q.x).fold(f32::MAX, f32::min),
                y0: p.iter().map(|q| q.y).fold(f32::MAX, f32::min),
                y1: p.iter().map(|q| q.y).fold(f32::MIN, f32::max),
                text: t.to_string(),
            })
        })
        .collect();
    regions.sort_by(|a, b| (a.y0 + a.y1).total_cmp(&(b.y0 + b.y1)));
    let mut lines: Vec<Vec<Region>> = Vec::new();
    for r in regions {
        let middle = (r.y0 + r.y1) / 2.0;
        match lines.last_mut() {
            Some(l)
                if {
                    let y0 = l.iter().map(|q| q.y0).fold(f32::MAX, f32::min);
                    let y1 = l.iter().map(|q| q.y1).fold(f32::MIN, f32::max);
                    middle > y0 && middle < y1
                } =>
            {
                l.push(r)
            }
            _ => lines.push(vec![r]),
        }
    }
    let mut heights: Vec<f32> = lines
        .iter()
        .map(|l| l.iter().map(|q| q.y1 - q.y0).fold(0.0, f32::max))
        .collect();
    heights.sort_by(f32::total_cmp);
    let usual = heights.get(heights.len() / 2).copied().unwrap_or(0.0);
    let mut out = String::new();
    let mut last_bottom: Option<f32> = None;
    for mut l in lines {
        l.sort_by(|a, b| a.x.total_cmp(&b.x));
        let y0 = l.iter().map(|q| q.y0).fold(f32::MAX, f32::min);
        let y1 = l.iter().map(|q| q.y1).fold(f32::MIN, f32::max);
        if let Some(b) = last_bottom {
            out.push('\n');
            if usual > 0.0 && y0 - b > 1.2 * usual {
                out.push('\n');
            }
        }
        out.push_str(
            &l.iter()
                .map(|q| q.text.as_str())
                .collect::<Vec<_>>()
                .join(" "),
        );
        last_bottom = Some(y1);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hyphenation_is_joined_only_before_lower_case() {
        assert_eq!(
            clean_page("die Verwal-\ntung des Landes"),
            "die Verwaltung\ndes Landes"
        );
        assert_eq!(clean_page("Bundes-\nLandesrecht"), "Bundes-\nLandesrecht");
        assert_eq!(clean_page("Satz -\nweiter"), "Satz -\nweiter");
    }

    #[test]
    fn blank_lines_and_form_feeds() {
        assert_eq!(clean_page("a\n\n\n\nb\u{0c}"), "a\n\nb");
    }

    #[test]
    fn heading_marker_is_escaped() {
        assert_eq!(clean_page("# not a heading"), "\\# not a heading");
    }

    #[test]
    fn control_and_format_characters_go() {
        assert_eq!(sanitize("scan\u{0}"), "scan");
        assert_eq!(
            sanitize("Ver\u{ad}waltung\tA\nB\u{feff}"),
            "Verwaltung\tA\nB"
        );
    }

    #[test]
    fn section_sign_gets_its_space() {
        assert_eq!(
            clean_page("(§133 Abs. 3) und §§ 1, 2 sowie § 5"),
            "(§ 133 Abs. 3) und §§ 1, 2 sowie § 5"
        );
    }
}
