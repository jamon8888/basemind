//! `redact_text` on a `file_path`, exercised through `basemind redact --file` (the CLI calls the
//! same tool method as the MCP server).
//!
//! Two properties:
//!
//! 1. **No clear-text copy on disk.** The extracted, not-yet-redacted text of the file must not be
//!    written anywhere the run can write: each test gives the child process its own `--root`,
//!    `XBERG_CACHE_DIR`, `BASEMIND_DATA_HOME` and temporary directory (`TMPDIR`, plus `TMP`/`TEMP`
//!    for Windows). xberg's cache directory must stay empty, and no file under any of the four may
//!    contain the invented party name or e-mail address of the fixtures. The bundled `.traineddata`
//!    files that `ensure_tessdata` writes under the data home are scanned like every other file: they
//!    hold no text of the input, so they pass the scan. The input files themselves live in a fifth
//!    directory that is not scanned.
//! 2. **OCR as document indexing runs it.** A scanned PDF and an image are read with the Tesseract
//!    data bundled in the binary, materialized under the test's data home, in the root's
//!    `[documents.ocr] languages`. Nothing is downloaded into xberg's cache.
//!
//! Fixtures (invented text, no metadata):
//! - `tests/fixtures/ocr/scan-fr.pdf`: image-only PDF (no text layer) of three French lines,
//!   rendered with Pillow (Arial 40 px, 1300x260, JPEG in PDF).
//! - `tests/fixtures/ocr/scan-fr.png`: the same three lines, rendered separately at 2400x480 and
//!   reduced to 1600x320 grayscale (66 KB). With the seven default languages Tesseract reads only
//!   the last line of this image (measured on a release build of this change), so the image test
//!   sets the root's languages to `fra` + `eng`, with which all three lines are read.
//!
//! Limit: these tests cannot hide a system tessdata directory (`/usr/local/share/tessdata`, ...)
//! from xberg's fallback search. The machine-independent proof that the bundled data is selected
//! is the unit test on the built configuration (`file_extraction_config_*` in
//! `src/mcp/redact_file_tests.rs`). Here, the language files materialized under the test's data
//! home and the empty xberg cache (no downloaded `tessdata/`) are the evidence. The accent check
//! only guards the reading: on this clean render, English data alone also reads `é` correctly.

#![cfg(feature = "documents")]

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

/// Invented strings from the fixtures that no file written by a run may contain.
const SENTINELS: &[&str] = &["ZORGLUB", "jeanne.fictive@example.com"];

/// The directories one `basemind redact --file` run is given, each private to one test.
struct Sandbox {
    /// `--root`.
    root: TempDir,
    /// `XBERG_CACHE_DIR`: xberg's whole-extraction and OCR result caches.
    xberg_cache: TempDir,
    /// `BASEMIND_DATA_HOME`: Basemind's cache root, where the bundled tessdata is materialized.
    data_home: TempDir,
    /// `TMPDIR`, `TMP` and `TEMP`.
    tmp: TempDir,
    /// Input files written by the test. Not scanned: the input itself holds the sentinels.
    inbox: TempDir,
}

impl Sandbox {
    fn new() -> Self {
        // Pins the comms and shells endpoints of the test process (inherited by the child) under an
        // isolated directory. The child's data home is then overridden per test in `redact_file`.
        basemind::store::init_isolated_cache();
        let dir = || tempfile::tempdir().expect("tempdir");
        Self {
            root: dir(),
            xberg_cache: dir(),
            data_home: dir(),
            tmp: dir(),
            inbox: dir(),
        }
    }

    /// The directories the run may write to.
    fn watched(&self) -> [&Path; 4] {
        [
            self.root.path(),
            self.xberg_cache.path(),
            self.data_home.path(),
            self.tmp.path(),
        ]
    }

    /// Where `ensure_tessdata` materializes the bundled language files for this run.
    fn tessdata(&self) -> PathBuf {
        self.data_home.path().join("cache/ocr/tessdata")
    }
}

/// Every entry (file or directory) under `dir`, recursively.
fn entries(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(current) = pending.pop() {
        let Ok(read) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in read.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path.clone());
            }
            found.push(path);
        }
    }
    found
}

/// Run `basemind --root <root> redact --file <file> --json [extra]` inside `sandbox`.
fn redact_file(sandbox: &Sandbox, file: &Path, extra: &[&str]) -> Value {
    let mut args = vec![
        "--root",
        sandbox.root.path().to_str().unwrap(),
        "redact",
        "--file",
        file.to_str().unwrap(),
        "--json",
    ];
    args.extend_from_slice(extra);
    let tmp = sandbox.tmp.path();
    let output = Command::new(env!("CARGO_BIN_EXE_basemind"))
        .args(args)
        .env("XBERG_CACHE_DIR", sandbox.xberg_cache.path())
        .env("BASEMIND_DATA_HOME", sandbox.data_home.path())
        .env("TMPDIR", tmp)
        .env("TMP", tmp)
        .env("TEMP", tmp)
        .output()
        .expect("run basemind redact");
    assert!(
        output.status.success(),
        "basemind redact failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("redact --json prints JSON")
}

/// Assert that xberg's cache directory is empty and that no file under the watched directories
/// contains a sentinel. Returns the files whose content was scanned.
fn assert_no_clear_text_on_disk(sandbox: &Sandbox) -> Vec<PathBuf> {
    let cached = entries(sandbox.xberg_cache.path());
    assert!(
        cached.is_empty(),
        "redact_text wrote under xberg's cache directory: {cached:#?}"
    );
    let mut scanned = Vec::new();
    let mut leaks = Vec::new();
    for dir in sandbox.watched() {
        for path in entries(dir) {
            if !path.is_file() {
                continue;
            }
            let bytes = std::fs::read(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
            for sentinel in SENTINELS {
                if memchr::memmem::find(&bytes, sentinel.as_bytes()).is_some() {
                    leaks.push(format!("{} contains {sentinel:?}", path.display()));
                }
            }
            scanned.push(path);
        }
    }
    assert!(leaks.is_empty(), "text of the input found on disk: {leaks:#?}");
    scanned
}

/// Assert that the bundled `languages` were materialized under the test's data home, and that the
/// content scan read them.
fn assert_bundled_tessdata(sandbox: &Sandbox, languages: &[&str], scanned: &[PathBuf]) {
    let tessdata = sandbox.tessdata();
    for language in languages {
        let file = tessdata.join(format!("{language}.traineddata"));
        assert!(
            file.is_file(),
            "bundled {language}.traineddata must be materialized under {}",
            tessdata.display()
        );
        assert!(scanned.contains(&file), "{} was not scanned", file.display());
    }
}

#[test]
fn redacting_a_file_leaves_no_extracted_text_on_disk() {
    let sandbox = Sandbox::new();
    let file = sandbox.inbox.path().join("note.md");
    std::fs::write(
        &file,
        "# Note fictive\n\nLa société ZORGLUB-FICTIF SARL a signé. \
         Écrire à jeanne.fictive@example.com avant la signature.\n",
    )
    .unwrap();

    let payload = redact_file(&sandbox, &file, &["--custom-term", "PARTY,ZORGLUB-FICTIF"]);
    let redacted = payload["redacted_text"].as_str().expect("redacted_text");
    assert!(!redacted.contains("jeanne.fictive@example.com"), "{redacted}");
    assert!(!redacted.contains("ZORGLUB"), "{redacted}");
    assert!(redacted.contains("avant la signature"), "{redacted}");

    assert_no_clear_text_on_disk(&sandbox);
}

#[test]
fn a_scanned_pdf_is_read_with_the_bundled_tessdata_and_leaves_nothing_on_disk() {
    let sandbox = Sandbox::new();
    let scan = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ocr/scan-fr.pdf");

    let payload = redact_file(&sandbox, &scan, &["--custom-term", "PARTY,ZORGLUB-FICTIF"]);
    let scanned = assert_no_clear_text_on_disk(&sandbox);

    let redacted = payload["redacted_text"].as_str().expect("redacted_text");
    assert!(!redacted.contains("ZORGLUB"), "the party must be redacted: {redacted}");
    let map = payload["rehydration_map"].to_string();
    assert!(map.contains("ZORGLUB-FICTIF"), "OCR must have read the party: {map}");
    assert!(
        redacted.contains("société") && redacted.contains("signé"),
        "accents must survive OCR: {redacted}"
    );
    // Default `[documents.ocr] languages`: all seven bundled files.
    assert_bundled_tessdata(&sandbox, &["eng", "deu", "fra", "spa", "ita", "nld", "pol"], &scanned);
}

#[test]
fn an_image_is_read_with_the_bundled_tessdata_and_leaves_nothing_on_disk() {
    let sandbox = Sandbox::new();
    // See the module docs: with the seven default languages this image loses two of its three
    // lines, so the root asks for French and English.
    std::fs::write(
        sandbox.root.path().join("basemind.toml"),
        "\"$schema\" = \"v1\"\n[documents.ocr]\nlanguages = [\"fra\", \"eng\"]\n",
    )
    .unwrap();
    let image = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ocr/scan-fr.png");

    let payload = redact_file(&sandbox, &image, &["--custom-term", "PARTY,ZORGLUB-FICTIF"]);
    let scanned = assert_no_clear_text_on_disk(&sandbox);

    let redacted = payload["redacted_text"].as_str().expect("redacted_text");
    assert!(!redacted.trim().is_empty(), "OCR must have read some text");
    assert!(!redacted.contains("ZORGLUB"), "the party must be redacted: {redacted}");
    let map = payload["rehydration_map"].to_string();
    assert!(map.contains("ZORGLUB-FICTIF"), "OCR must have read the party: {map}");
    assert_bundled_tessdata(&sandbox, &["fra", "eng"], &scanned);
}
