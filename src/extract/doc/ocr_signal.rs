//! #95: the near-empty-OCR signal. An image whose OCR comes back empty or near-empty is a
//! silent failure — seen with images carrying an alpha channel (decoded without being
//! composited onto white) and with pages whose lines the OCR dictionary filter discards.
//! `extract_doc` turns it into a warning and an `ocr_empty` metadata note.

use std::path::Path;

use super::DocumentModelProfile;

/// Fewer non-whitespace characters than this on an image OCR is "near-empty".
const NEAR_EMPTY_OCR_CHARS: usize = 20;

/// True when an image's OCR output is empty or near-empty. A pure predicate, unit-testable
/// without the xberg extraction runtime.
pub(super) fn ocr_output_near_empty(mime_type: Option<&str>, content: &str) -> bool {
    // SVG is text markup, extracted without OCR.
    mime_type.is_some_and(|m| m.starts_with("image/") && m != "image/svg+xml")
        && non_whitespace_chars(content) < NEAR_EMPTY_OCR_CHARS
}

/// The non-whitespace character count of a near-empty image OCR, logged as a warning; `None`
/// when the profile ran no OCR (`CodeOnly`, `None_`) or the OCR read enough text.
pub(super) fn check(path: &Path, mime_type: Option<&str>, profile: &DocumentModelProfile, content: &str) -> Option<usize> {
    let ocr_ran = !matches!(profile, DocumentModelProfile::CodeOnly | DocumentModelProfile::None_);
    if !(ocr_ran && ocr_output_near_empty(mime_type, content)) {
        return None;
    }
    let ocr_chars = non_whitespace_chars(content);
    tracing::warn!(
        path = %path.display(),
        mime = mime_type.unwrap_or(""),
        ocr_chars,
        "OCR of an image returned no (or very little) text; the page may be blank, low-resolution, or unreadable by the OCR engine"
    );
    Some(ocr_chars)
}

/// The `ocr_empty` metadata note carried by the document.
pub(super) fn note(ocr_chars: usize) -> (String, String) {
    (
        "ocr_empty".to_string(),
        format!(
            "OCR returned {ocr_chars} non-whitespace characters from an image; the page may be blank, low-resolution, or unreadable by the OCR engine"
        ),
    )
}

fn non_whitespace_chars(content: &str) -> usize {
    content.chars().filter(|c| !c.is_whitespace()).count()
}
