# Documents: PDFs and scans in the store

`cyberbrain documents` turns PDFs into notes: one note per document, every page under its own
locator, so a recall hit says which page to open (`[at p. 7]`). Pages that are only an image —
a scanned letter, a fax — are recognised (OCR) when the models are installed.

It is a build feature, off by default, because it brings ONNX Runtime and PDFium bindings and a
larger dependency tree that memory for coding agents does not need:

```sh
cargo install cyberbrain --features documents
```

## What it needs, and where it goes

Nothing is downloaded at run time. The files go into the store's `models/` folder, next to the
embedding model, so an office without internet installs them by copying a folder.

| Folder | File | From |
|---|---|---|
| `models/pdfium/` | `pdfium.dll` / `libpdfium.dylib` / `libpdfium.so` | [bblanchon/pdfium-binaries](https://github.com/bblanchon/pdfium-binaries/releases) (PDFium: BSD-3/Apache-2.0; build: MIT) |
| `models/onnxruntime/` | `onnxruntime.dll` / `libonnxruntime.dylib` / `libonnxruntime.so` | [microsoft/onnxruntime releases](https://github.com/microsoft/onnxruntime/releases) (MIT); tested with 1.31.1 |
| `models/ppocrv6/` | `pp-ocrv6_small_det.onnx`, `pp-ocrv6_small_rec.onnx`, `ppocrv6_dict.txt` | PaddlePaddle on Hugging Face, `PP-OCRv6_small_det_onnx` and `PP-OCRv6_small_rec_onnx` (Apache-2.0); the dictionary is the `character_dict` of the recognition model's `inference.yml`, one entry per line |

PDFium is required. Without ONNX Runtime or the PP-OCRv6 files, text layers are still read and
pages that are only an image are reported as empty — never silently as an empty document.

## Use

```sh
cyberbrain documents add contracts/ --bereich matter-2026-0142
cyberbrain documents list
cyberbrain recall "Wiedereinsetzung Frist" --bereich matter-2026-0142
cyberbrain forget doc-schriftsatz-mueller      # removal, with the Art. 17 report
```

- A file that has not changed since the last run is skipped (`unchanged`); one that has changed
  replaces its note. A note of the same name that did not come from that file is never
  overwritten.
- Every note goes through the normal write path: the PII gate, the audit row, the index. A
  document with personal data is held like any other note; only a person can mark a finding as
  reviewed (`--accept-pii` is refused to an agent).
- Notes land in ring 4 (imported material) as kind `reference`, tagged with the source file
  and the hash of the original bytes.
- `--threads` sets how many documents are read at once. Each recognition pipeline holds its
  models in memory, about 0.7 GB with the small models; on a laptop this is the memory limit.
- The web UI lists documents but does not add them: adding reads files wherever it is pointed,
  and that surface has no authentication.

## What was measured

1,400 decisions of the German federal courts (official works, § 5 UrhG; 10,171 pages) and 200
scanned pages simulated from them (200 dpi, rotated up to ±1.5°, blurred, noise, JPEG). The
simulated scans are cleaner than real paper: stamps, faxes and handwriting are not in it.

| 200 scanned pages | Tesseract 5.3 (`Latin`) | PP-OCRv6 small, 120 dpi |
|---|---|---|
| character error rate (mean of 20 pages) | 0.98 % | 0.18 % |
| word error rate | 2.5 % | 0.67 % |
| `§` kept | 732 of 750 | 750 of 750 |
| citations like `§ 58 Abs. 3 Nr. 1` exact | 257 of 262 | 261 of 262 |
| pages/s, 12 threads / 4 threads | 0.9 / 0.4 | 2.3 / 1.0 |

Three findings shaped the code, each with a test that failed before the fix:

- Tesseract's German model has no `§` in its character set and reads every one as `$`.
- PP-OCRv6 drops the space between words when the page image is too fine: 7.9 % of word
  boundaries at 300 dpi, 0.03 % at 120 dpi. Pages are rendered at 120 dpi.
- PDFium's page text has no blank lines between paragraphs; blocks are cut at paragraphs, so
  they are rebuilt from the line advance (22,109 blocks against Poppler's 22,150).

## Limits

- PDF only for now. DOCX and images on their own are not read yet.
- Reading the text layer goes through one lock (PDFium is not thread-safe): about 300 pages/s
  regardless of threads.
- Layouts with several columns are read column by column only as far as the PDF's own order
  allows.
