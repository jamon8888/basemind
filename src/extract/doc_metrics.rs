//! Stable document-extraction entry point for callers that do not need scan metrics.

use super::ExtractError;
use super::doc::{DocConfig, FileMapDoc, extract_doc_with_metrics};

/// Extract one document without attaching it to a scan metrics accumulator.
pub fn extract_doc(
    path: &std::path::Path,
    mime_type: Option<&str>,
    config: &DocConfig,
) -> Result<FileMapDoc, ExtractError> {
    extract_doc_with_metrics(path, mime_type, config, None)
}
