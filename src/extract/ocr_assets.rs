//! Tesseract language data bundled into document-feature builds.
//!
//! The build script downloads and verifies the pinned upstream `tessdata_fast` files. At runtime
//! they are materialized under Basemind's own cache root so xberg never needs to reach the network
//! for the default OCR languages.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const ENG: &[u8] = include_bytes!(env!("BASEMIND_BUNDLED_TESSDATA_ENG"));
const DEU: &[u8] = include_bytes!(env!("BASEMIND_BUNDLED_TESSDATA_DEU"));
const FRA: &[u8] = include_bytes!(env!("BASEMIND_BUNDLED_TESSDATA_FRA"));
const SPA: &[u8] = include_bytes!(env!("BASEMIND_BUNDLED_TESSDATA_SPA"));
const ITA: &[u8] = include_bytes!(env!("BASEMIND_BUNDLED_TESSDATA_ITA"));
const NLD: &[u8] = include_bytes!(env!("BASEMIND_BUNDLED_TESSDATA_NLD"));
const POL: &[u8] = include_bytes!(env!("BASEMIND_BUNDLED_TESSDATA_POL"));

const BUNDLED: &[(&str, &[u8])] = &[
    ("eng", ENG),
    ("deu", DEU),
    ("fra", FRA),
    ("spa", SPA),
    ("ita", ITA),
    ("nld", NLD),
    ("pol", POL),
];

static TESSDATA: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();

/// Whether the embedded directory contains every language in a document OCR configuration.
pub(crate) fn can_bundle_languages(languages: &[String]) -> bool {
    languages.iter().all(|language| {
        language
            .split('+')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .all(|part| BUNDLED.iter().any(|(bundled, _)| bundled.eq_ignore_ascii_case(part)))
    })
}

/// Return the Basemind-owned directory containing the bundled Tesseract language files.
pub(crate) fn tessdata_path() -> PathBuf {
    crate::store_layout::cache_root()
        .join(crate::store_layout::CACHE_DIR)
        .join("ocr/tessdata")
}

/// Materialize the embedded language files once per process and return their directory.
pub(crate) fn ensure_tessdata() -> Result<PathBuf, String> {
    let cache = TESSDATA.get_or_init(|| Mutex::new(None));
    let mut cached = cache.lock().map_err(|_| "OCR tessdata cache poisoned".to_string())?;
    if let Some(directory) = cached.as_ref() {
        return Ok(directory.clone());
    }
    let directory = tessdata_path();
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("create OCR tessdata directory {}: {error}", directory.display()))?;
    for &(language, bytes) in BUNDLED {
        ensure_file(&directory.join(format!("{language}.traineddata")), bytes)?;
    }
    *cached = Some(directory.clone());
    Ok(directory)
}

fn ensure_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if std::fs::read(path).is_ok_and(|cached| cached == bytes) {
        return Ok(());
    }

    let temporary = path.with_extension(format!("traineddata.tmp-{}", std::process::id()));
    std::fs::write(&temporary, bytes)
        .map_err(|error| format!("write temporary OCR data {}: {error}", temporary.display()))?;
    if let Err(error) = std::fs::rename(&temporary, path)
        && !std::fs::read(path).is_ok_and(|cached| cached == bytes)
    {
        return Err(format!("install OCR data {}: {error}", path.display()));
    }
    Ok(())
}

/// The tessdata directory document OCR reads `languages` from: the bundled one when it holds every
/// language, `None` otherwise (xberg then resolves the language files itself).
pub fn bundled_tessdata_for(languages: &[String]) -> Result<Option<PathBuf>, String> {
    if can_bundle_languages(languages) {
        ensure_tessdata().map(Some)
    } else {
        Ok(None)
    }
}

/// xberg OCR settings for `languages`, read from `tessdata_path`. Shared by document indexing and
/// `redact_text` so a file is OCR'd the same way by both.
pub fn xberg_ocr_config(languages: &[String], tessdata_path: Option<&Path>) -> xberg::core::config::OcrConfig {
    xberg::core::config::OcrConfig {
        language: languages.to_vec(),
        tessdata_path: tessdata_path.map(Path::to_path_buf),
        ..Default::default()
    }
}

fn ensure_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if std::fs::read(path).is_ok_and(|cached| cached == bytes) {
        return Ok(());
    }

    let temporary = path.with_extension(format!("traineddata.tmp-{}", std::process::id()));
    std::fs::write(&temporary, bytes)
        .map_err(|error| format!("write temporary OCR data {}: {error}", temporary.display()))?;
    if let Err(error) = std::fs::rename(&temporary, path)
        && !std::fs::read(path).is_ok_and(|cached| cached == bytes)
    {
        return Err(format!("install OCR data {}: {error}", path.display()));
    }
    Ok(())
}
