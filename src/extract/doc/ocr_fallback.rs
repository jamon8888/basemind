//! Narrow OCR retry for xberg's lossy dictionary filter.

use std::collections::HashSet;

use super::ocr_signal;
use xberg::core::config::ExtractionConfig;

/// Build the OCR retry configuration used when xberg reports that its dictionary filter removed
/// lines without finding a valid dictionary word. Normal documents keep xberg's markdown/table
/// path; only the affected document retries through plain text.
pub(crate) fn plain_text_ocr_fallback_config(config: &ExtractionConfig) -> Option<ExtractionConfig> {
    let mut fallback = config.clone();
    let ocr = fallback.ocr.get_or_insert_with(Default::default);
    let tesseract = ocr
        .tesseract_config
        .get_or_insert_with(xberg::types::TesseractConfig::default);
    tesseract.output_format = "text".to_string();
    tesseract.enable_table_detection = false;
    Some(fallback)
}

const TESSERACT_DICT_INVALID_WORD_RATIO_KEY: &str = "tesseract_dict_invalid_word_ratio";

fn has_no_dictionary_signal(result: &xberg::types::ExtractedDocument) -> bool {
    result
        .metadata
        .additional
        .get(TESSERACT_DICT_INVALID_WORD_RATIO_KEY)
        .and_then(serde_json::Value::as_f64)
        .is_some_and(|ratio| ratio >= 1.0)
}

/// Return warnings proving xberg's lossy dictionary filter ran. They are copied onto a
/// successful plain-text retry so Basemind's telemetry retains the original loss signal.
pub(crate) fn dictionary_filter_warnings(
    outcome: &xberg::Result<xberg::ExtractionResult>,
) -> Option<Vec<xberg::types::ProcessingWarning>> {
    let extraction = outcome.as_ref().ok()?;
    dictionary_filter_warnings_from_extraction(extraction)
}

pub(crate) fn dictionary_filter_warnings_from_extraction(
    extraction: &xberg::ExtractionResult,
) -> Option<Vec<xberg::types::ProcessingWarning>> {
    let warnings = extraction
        .results
        .iter()
        .filter(|result| has_no_dictionary_signal(result))
        .flat_map(|result| {
            result
                .processing_warnings
                .iter()
                .filter(|warning| ocr_signal::dictionary_filtered_line_count(std::slice::from_ref(*warning)).is_some())
                .cloned()
        })
        .collect::<Vec<_>>();
    (!warnings.is_empty()).then_some(warnings)
}

pub(crate) fn preserve_dictionary_filter_warnings(
    extraction: &mut xberg::ExtractionResult,
    warnings: Vec<xberg::types::ProcessingWarning>,
) {
    if let Some(result) = extraction.results.last_mut() {
        result.processing_warnings.extend(warnings);
    }
}

/// Keep text that the structured OCR pass recovered when the plain-text retry omits it (for
/// example, numeric cells that were present only in the table-aware output).
pub(crate) fn preserve_original_ocr_text(original: &xberg::ExtractionResult, fallback: &mut xberg::ExtractionResult) {
    for (index, original) in original.results.iter().enumerate() {
        let Some(fallback) = fallback.results.get_mut(index) else {
            break;
        };
        let missing = {
            let existing_lines = fallback
                .content
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .collect::<HashSet<_>>();
            let mut seen = HashSet::new();
            let mut missing = Vec::new();
            for line in original
                .content
                .lines()
                .chain(original.tables.iter().flat_map(|table| table.markdown.lines()))
            {
                let line = line.trim();
                if !line.is_empty() && !existing_lines.contains(line) && seen.insert(line) {
                    missing.push(line);
                }
            }
            missing
        };
        if missing.is_empty() {
            continue;
        }
        if !fallback.content.is_empty() {
            fallback.content.push_str("\n\n");
        }
        fallback.content.push_str(&missing.join("\n"));
    }
}
