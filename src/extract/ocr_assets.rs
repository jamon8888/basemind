//! Tesseract language data bundled into document-feature builds.
//!
//! The build script downloads and verifies the pinned upstream `tessdata_fast` files. At runtime
//! they are materialized under Basemind's own cache root so xberg never needs to reach the network
//! for the default OCR languages.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

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

static TESSDATA: OnceLock<Result<PathBuf, String>> = OnceLock::new();

/// Return the Basemind-owned directory containing the bundled Tesseract language files.
pub(crate) fn tessdata_path() -> PathBuf {
    crate::store_layout::cache_root()
        .join(crate::store_layout::CACHE_DIR)
        .join("ocr/tessdata")
}

/// Materialize the embedded language files once per process and return their directory.
pub(crate) fn ensure_tessdata() -> Result<PathBuf, String> {
    TESSDATA
        .get_or_init(|| {
            let directory = tessdata_path();
            std::fs::create_dir_all(&directory)
                .map_err(|error| format!("create OCR tessdata directory {}: {error}", directory.display()))?;
            for &(language, bytes) in BUNDLED {
                ensure_file(&directory.join(format!("{language}.traineddata")), bytes)?;
            }
            Ok(directory)
        })
        .clone()
}

fn ensure_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if std::fs::metadata(path).is_ok_and(|metadata| metadata.len() == bytes.len() as u64) {
        return Ok(());
    }

    let temporary = path.with_extension(format!("traineddata.tmp-{}", std::process::id()));
    std::fs::write(&temporary, bytes)
        .map_err(|error| format!("write temporary OCR data {}: {error}", temporary.display()))?;
    if let Err(error) = std::fs::rename(&temporary, path)
        && !std::fs::metadata(path).is_ok_and(|metadata| metadata.len() == bytes.len() as u64)
    {
        return Err(format!("install OCR data {}: {error}", path.display()));
    }
    Ok(())
}
