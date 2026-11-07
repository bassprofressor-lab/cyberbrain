//! Needs the PDFium library and the PP-OCRv6 files:
//!   CYBERBRAIN_PDFIUM_DIR=<folder with libpdfium> CYBERBRAIN_PPOCRV6_DIR=<folder with pp-ocrv6_*> \
//!   CYBERBRAIN_ORT_DIR=<folder with the ONNX Runtime library> \
//!   cargo test -p cyberbrain-documents --release -- --ignored
//! Without them these tests fail with that instruction instead of passing silently.
//!
//! Test data: BVerwG, decision of 08.01.2010, 9 B 3/09 (an official work, § 5 UrhG), once with
//! a text layer and once rasterised, slightly rotated and noisy, without one; two decisions
//! whose last page holds only the judges' names or nothing.
use cyberbrain_core::source::Document;
use cyberbrain_documents::{
    ModelSize, PageRoute, PdfSource, Read, load_onnx_runtime, ocr_pipeline, onnx_runtime_file,
};
use std::path::PathBuf;
use std::sync::OnceLock;

fn source() -> &'static PdfSource {
    static S: OnceLock<PdfSource> = OnceLock::new();
    S.get_or_init(|| {
        let lib = std::env::var("CYBERBRAIN_PDFIUM_DIR")
            .expect("set CYBERBRAIN_PDFIUM_DIR to the folder with libpdfium");
        let models = std::env::var("CYBERBRAIN_PPOCRV6_DIR")
            .expect("set CYBERBRAIN_PPOCRV6_DIR to the folder with the PP-OCRv6 files");
        let ort = std::env::var("CYBERBRAIN_ORT_DIR")
            .expect("set CYBERBRAIN_ORT_DIR to the folder with the ONNX Runtime library");
        load_onnx_runtime(&onnx_runtime_file(&PathBuf::from(ort))).unwrap();
        let ocr = ocr_pipeline(&PathBuf::from(models), ModelSize::Small, 1).unwrap();
        PdfSource::new(&PathBuf::from(lib), vec![ocr]).unwrap()
    })
}

fn read(file: &str) -> Read {
    let path = format!("{}/tests/data/{file}", env!("CARGO_MANIFEST_DIR"));
    let bytes = std::fs::read(&path).unwrap();
    source()
        .read(&Document {
            name: file,
            media_type: None,
            bytes: &bytes,
        })
        .unwrap()
}

#[test]
#[ignore = "needs CYBERBRAIN_PDFIUM_DIR, CYBERBRAIN_PPOCRV6_DIR and CYBERBRAIN_ORT_DIR"]
fn text_layer_with_page_locators() {
    let r = read("decision-text-layer.pdf");
    assert_eq!(r.pages.len(), 3);
    assert!(
        r.pages.iter().all(|p| p.route == PageRoute::TextLayer),
        "{:?}",
        r.pages
    );
    assert!(
        r.extracted
            .body
            .contains("<!-- cyberbrain:locator p. 3 -->")
    );
    assert!(
        r.extracted
            .body
            .contains("Die Beschwerde ist als unzulässig zu verwerfen")
    );
}

/// Without paragraphs page 1 would be a single block.
#[test]
#[ignore = "needs CYBERBRAIN_PDFIUM_DIR, CYBERBRAIN_PPOCRV6_DIR and CYBERBRAIN_ORT_DIR"]
fn text_layer_keeps_paragraphs() {
    let r = read("decision-text-layer.pdf");
    let page1 = r
        .extracted
        .body
        .split("<!-- cyberbrain:locator p. 2 -->")
        .next()
        .unwrap();
    let paragraphs = page1.split("\n\n").filter(|p| !p.trim().is_empty()).count();
    assert!(
        paragraphs >= 6,
        "only {paragraphs} paragraphs on page 1:\n{page1}"
    );
    assert!(page1.contains("Tenor\n\nDie Beschwerde"), "{page1}");
}

/// The section sign survives the scan: Tesseract's German model has no `§` and read every one
/// as `$`, which is how this test came to exist.
#[test]
#[ignore = "needs CYBERBRAIN_PDFIUM_DIR, CYBERBRAIN_PPOCRV6_DIR and CYBERBRAIN_ORT_DIR"]
fn scan_is_recognised_with_its_citations() {
    let r = read("decision-scanned.pdf");
    assert!(
        r.pages.iter().all(|p| p.route == PageRoute::Ocr),
        "{:?}",
        r.pages
    );
    for t in [
        "§ 133 Abs. 3",
        "§ 60 Abs. 1",
        "Streitgegenstandes",
        "Nichtzulassung",
    ] {
        assert!(
            r.extracted.body.contains(t),
            "{t} missing:\n{}",
            r.extracted.body
        );
    }
}

#[test]
#[ignore = "needs CYBERBRAIN_PDFIUM_DIR, CYBERBRAIN_PPOCRV6_DIR and CYBERBRAIN_ORT_DIR"]
fn short_or_empty_text_pages_are_not_recognised() {
    let r = read("short-last-page.pdf");
    assert_eq!(r.pages[1].route, PageRoute::TextLayer, "{:?}", r.pages);
    let r = read("empty-last-page.pdf");
    assert_eq!(
        r.pages.last().unwrap().route,
        PageRoute::Empty,
        "{:?}",
        r.pages
    );
}

#[test]
#[ignore = "needs CYBERBRAIN_PDFIUM_DIR, CYBERBRAIN_PPOCRV6_DIR and CYBERBRAIN_ORT_DIR"]
fn no_control_characters_reach_the_note() {
    let r = read("decision-scanned.pdf");
    let title = r.extracted.title.unwrap_or_default();
    for t in [&title, &r.extracted.body] {
        assert!(
            !t.chars().any(|c| c.is_control() && c != '\n' && c != '\t'),
            "{t:?}"
        );
    }
}
