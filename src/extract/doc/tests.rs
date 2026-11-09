use std::borrow::Cow;

use super::*;
use xberg::types::{HeadingContext, HeadingLevel};

/// Build a `DocConfig` with every enrichment post-processor turned on so a
/// profile that strips them is observable in `to_xberg`'s output.
fn doc_config_all_enrichment_on(profile: DocumentModelProfile) -> DocConfig {
    DocConfig {
        keywords: KeywordsConfig {
            enabled: true,
            ..KeywordsConfig::default()
        },
        ner: NerConfig {
            enabled: true,
            ..NerConfig::default()
        },
        summarization: SummarizationConfig {
            enabled: true,
            ..SummarizationConfig::default()
        },
        document_models: profile,
        ..DocConfig::default()
    }
}

#[test]
fn to_xberg_full_profile_keeps_enrichment_and_ocr() {
    let cfg = doc_config_all_enrichment_on(DocumentModelProfile::Full);
    let x = cfg.to_xberg(None);
    assert!(x.keywords.is_some(), "Full keeps keyword extraction");
    assert!(x.ner.is_some(), "Full keeps NER");
    assert!(x.summarization.is_some(), "Full keeps summarisation");
    assert!(!x.disable_ocr, "Full leaves OCR enabled");
    assert_eq!(x.ocr.as_ref().unwrap().language, cfg.ocr_languages);
    assert!(x.ocr.as_ref().unwrap().tessdata_path.is_none());
    assert!(
        x.ocr.as_ref().unwrap().tesseract_config.is_none(),
        "healthy OCR keeps xberg's normal markdown/table path"
    );
    assert!(x.chunking.as_ref().and_then(|c| c.embedding.as_ref()).is_none());
}

#[test]
fn dictionary_filter_retry_switches_only_the_affected_document_to_plain_ocr() {
    let cfg = doc_config_all_enrichment_on(DocumentModelProfile::Full).to_xberg(None);
    let fallback = super::plain_text_ocr_fallback_config(&cfg).expect("OCR must be configured");
    let tesseract = fallback.ocr.unwrap().tesseract_config.unwrap();
    assert_eq!(tesseract.output_format, "text");
    assert!(!tesseract.enable_table_detection);
}

#[test]
fn dictionary_filter_retry_requires_a_page_with_no_valid_dictionary_words() {
    let warning = xberg::types::ProcessingWarning {
        source: "tesseract".into(),
        message: "Tesseract removed 33 OCR line(s) because their dictionary-checkable words were mostly not real words"
            .into(),
    };
    let mut document = xberg::types::ExtractedDocument::default();
    document.processing_warnings.push(warning.clone());
    document
        .metadata
        .additional
        .insert("tesseract_dict_invalid_word_ratio".into(), serde_json::json!(1.0));
    let outcome = Ok(xberg::ExtractionResult::single(document.clone()));
    assert!(super::dictionary_filter_warnings(&outcome).is_some());

    document
        .metadata
        .additional
        .insert("tesseract_dict_invalid_word_ratio".into(), serde_json::json!(0.75));
    let outcome = Ok(xberg::ExtractionResult::single(document));
    assert!(super::dictionary_filter_warnings(&outcome).is_none());
}

#[test]
fn plain_text_retry_preserves_lines_recovered_by_structured_ocr() {
    let mut original = xberg::types::ExtractedDocument::default();
    original.content = "Name\nInvoice total: 42".into();
    let mut fallback = xberg::types::ExtractedDocument::default();
    fallback.content = "Name".into();

    let original = xberg::ExtractionResult::single(original);
    let mut fallback = xberg::ExtractionResult::single(fallback);
    super::preserve_original_ocr_text(&original, &mut fallback);

    assert!(fallback.results[0].content.contains("Invoice total: 42"));
}

#[test]
fn plain_text_retry_reconciles_each_result_and_keeps_short_lines() {
    let warning = xberg::types::ProcessingWarning {
        source: "tesseract".into(),
        message: "Tesseract removed 1 OCR line(s) because their dictionary-checkable words were mostly not real words"
            .into(),
    };
    let mut original_first = xberg::types::ExtractedDocument::default();
    original_first.content = "Yes".into();
    original_first.processing_warnings.push(warning.clone());
    original_first
        .metadata
        .additional
        .insert("tesseract_dict_invalid_word_ratio".into(), serde_json::json!(1.0));
    let mut original_second = xberg::types::ExtractedDocument::default();
    original_second.content = "Invoice total: 42".into();
    let original = xberg::ExtractionResult {
        results: vec![original_first, original_second],
        ..xberg::ExtractionResult::default()
    };

    let mut fallback_first = xberg::types::ExtractedDocument::default();
    fallback_first.content = "Yesterday".into();
    let mut fallback_second = xberg::types::ExtractedDocument::default();
    fallback_second.content = "Invoice".into();
    let mut fallback = xberg::ExtractionResult {
        results: vec![fallback_first, fallback_second],
        ..xberg::ExtractionResult::default()
    };

    let warnings = super::ocr_fallback::dictionary_filter_warnings_from_extraction(&original)
        .expect("first result triggers retry");
    super::preserve_original_ocr_text(&original, &mut fallback);
    super::preserve_dictionary_filter_warnings(&mut fallback, warnings);

    assert!(fallback.results[0].content.contains("Yes"));
    assert!(fallback.results[1].content.contains("Invoice total: 42"));
    assert_eq!(fallback.results[1].processing_warnings.len(), 1);
    assert_eq!(fallback.results[1].processing_warnings[0].message, warning.message);
}

#[test]
fn to_xberg_code_only_strips_enrichment_disables_ocr_keeps_embeddings() {
    let cfg = doc_config_all_enrichment_on(DocumentModelProfile::CodeOnly);
    let x = cfg.to_xberg(None);
    assert!(x.keywords.is_none(), "CodeOnly forces keywords off");
    assert!(x.ner.is_none(), "CodeOnly forces NER off");
    assert!(x.summarization.is_none(), "CodeOnly forces summarisation off");
    assert!(x.disable_ocr, "CodeOnly disables OCR");
    assert!(x.ocr.is_none(), "CodeOnly does not configure OCR");
    assert!(x.chunking.as_ref().and_then(|c| c.embedding.as_ref()).is_none());
}

#[test]
fn to_xberg_none_profile_strips_everything_including_embeddings() {
    let cfg = doc_config_all_enrichment_on(DocumentModelProfile::None_);
    let x = cfg.to_xberg(None);
    assert!(x.keywords.is_none(), "None strips keywords");
    assert!(x.ner.is_none(), "None strips NER");
    assert!(x.summarization.is_none(), "None strips summarisation");
    assert!(x.disable_ocr, "None disables OCR");
    assert!(x.ocr.is_none(), "None does not configure OCR");
    assert!(
        x.chunking.as_ref().and_then(|c| c.embedding.as_ref()).is_none(),
        "None skips embeddings entirely even when embed = true"
    );
}

#[test]
fn to_xberg_defers_embedding_to_basemind() {
    let cfg = DocConfig {
        embed: true,
        embed_batch_size: 8,
        ..DocConfig::default()
    };
    let x = cfg.to_xberg(None);
    assert!(
        x.chunking.as_ref().and_then(|c| c.embedding.as_ref()).is_none(),
        "basemind must embed the breadcrumbed retrieval view after extraction"
    );
}

#[test]
fn to_xberg_bounds_pages_timeout_and_disables_duplicate_cache() {
    let cfg = DocConfig {
        max_pages: 37,
        extraction_timeout_secs: 42,
        ..doc_config_all_enrichment_on(DocumentModelProfile::Full)
    };

    let xberg = cfg.to_xberg(None);
    let security = xberg.security_limits.expect("security limits configured");
    let chunking = xberg.chunking.expect("chunking configured");

    assert!(
        !xberg.use_cache,
        "basemind's content-addressed blob cache is the only extraction cache"
    );
    assert_eq!(xberg.extraction_timeout_secs, Some(42));
    assert_eq!(security.max_pages, Some(37));
    assert_eq!(chunking.chunker_type, ChunkerType::Markdown);
}

#[test]
fn dense_retrieval_text_adds_breadcrumb_without_changing_source() {
    let source = "## Setup\n\nInstall dependencies.".to_string();
    let context = HeadingContext {
        headings: vec![
            HeadingLevel {
                level: 1,
                text: "Guide".to_string(),
            },
            HeadingLevel {
                level: 2,
                text: "Setup".to_string(),
            },
        ],
    };

    let (stored, retrieval) = prepare_doc_chunk(source, 10, 44, Some(&context), true);

    assert_eq!(stored.text, "## Setup\n\nInstall dependencies.");
    assert_eq!(stored.byte_start, 10);
    assert_eq!(stored.byte_end, 44);
    assert_eq!(
        retrieval.as_deref(),
        Some("# Guide > ## Setup\n\nInstall dependencies.")
    );
}

#[test]
fn dense_retrieval_text_borrows_exact_source_without_heading_context() {
    let retrieval = dense_retrieval_text("Exact source span", None);

    assert!(matches!(retrieval, Cow::Borrowed("Exact source span")));
}

#[test]
fn embedding_disabled_keeps_exact_text_without_allocating_retrieval_input() {
    let (stored, retrieval) = prepare_doc_chunk("Exact source span".to_string(), 0, 17, None, false);

    assert_eq!(stored.text, "Exact source span");
    assert!(retrieval.is_none());
}

#[test]
fn language_confidences_are_persisted_without_xberg_types() {
    let mapped = map_language_confidences(vec![xberg::types::LanguageConfidence {
        language: "eng".to_string(),
        confidence: 0.98,
        proportion: 0.75,
        script: "Latin".to_string(),
        reliable: true,
    }]);

    assert_eq!(
        mapped,
        vec![DocLanguageConfidence {
            language: "eng".to_string(),
            confidence: 0.98,
            proportion: 0.75,
            script: "Latin".to_string(),
            reliable: true,
        }]
    );
}

/// #95: the near-empty-OCR signal fires only for an image whose OCR came back
/// empty (or near-empty) — never for a non-image format, and never for an image
/// that produced real text.
#[test]
fn ocr_near_empty_signal_is_image_scoped_and_thresholded() {
    // A non-image format is never flagged, even when blank.
    assert!(!ocr_signal::ocr_output_near_empty(Some("application/pdf"), ""));
    assert!(!ocr_signal::ocr_output_near_empty(Some("text/plain"), "   "));

    // An image with real OCR output is not flagged.
    assert!(!ocr_signal::ocr_output_near_empty(
        Some("image/png"),
        "contrat de prestation de services conclu entre deux parties"
    ));

    // An image whose OCR is empty, whitespace-only, or a near-empty fragment is.
    assert!(ocr_signal::ocr_output_near_empty(Some("image/png"), ""));
    assert!(ocr_signal::ocr_output_near_empty(Some("image/jpeg"), "   \n\n  \t"));
    assert!(ocr_signal::ocr_output_near_empty(Some("image/png"), "Le  "));

    // SVG is text markup, not OCR output.
    assert!(!ocr_signal::ocr_output_near_empty(Some("image/svg+xml"), ""));

    // No mime type (defensive) is not treated as an image.
    assert!(!ocr_signal::ocr_output_near_empty(None, ""));

    // The floor counts non-whitespace characters: 19 is near-empty, 20 is not.
    assert!(ocr_signal::ocr_output_near_empty(Some("image/png"), &"a ".repeat(19)));
    assert!(!ocr_signal::ocr_output_near_empty(Some("image/png"), &"a ".repeat(20)));
}

/// #95: the signal follows the profile — no OCR ran under `CodeOnly` / `None_`, so a blank
/// image is not reported as an OCR failure there.
#[test]
fn ocr_near_empty_signal_is_silent_when_the_profile_ran_no_ocr() {
    let png = std::path::Path::new("scan.png");
    assert_eq!(
        ocr_signal::check(png, Some("image/png"), &DocumentModelProfile::Full, " a "),
        Some(1)
    );
    assert_eq!(
        ocr_signal::check(png, Some("image/png"), &DocumentModelProfile::CodeOnly, ""),
        None
    );
    assert_eq!(
        ocr_signal::check(png, Some("image/png"), &DocumentModelProfile::None_, ""),
        None
    );
    assert_eq!(
        ocr_signal::check(png, Some("image/png"), &DocumentModelProfile::Full, &"a ".repeat(20)),
        None
    );
}

#[test]
fn transparent_png_is_composited_over_white_before_ocr() {
    let file = tempfile::NamedTempFile::with_suffix(".png").expect("temp PNG");
    let mut png = Vec::new();
    let image = image::RgbaImage::from_pixel(1, 1, image::Rgba([0, 0, 0, 0]));
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .expect("encode transparent PNG");
    std::fs::write(file.path(), png).expect("write transparent PNG");

    let input = super::prepare_extraction_input(
        file.path(),
        Some("image/png"),
        xberg::extractors::security::SecurityLimits::default().max_content_size,
    )
    .expect("normalize PNG");
    let bytes = input.bytes.expect("normalized input uses bytes");
    let decoded = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
        .expect("decode normalized PNG")
        .to_rgb8();
    assert_eq!(decoded.get_pixel(0, 0).0, [255, 255, 255]);
}

#[test]
fn oversized_png_is_rejected_before_rgba_allocation() {
    let file = tempfile::NamedTempFile::with_suffix(".png").expect("temp PNG");
    let mut png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR".to_vec();
    png.extend_from_slice(&200_000u32.to_be_bytes());
    png.extend_from_slice(&200_000u32.to_be_bytes());
    png.extend_from_slice(&[8, 6, 0, 0, 0]);
    png.extend_from_slice(&0xf93b880au32.to_be_bytes());
    png.extend_from_slice(&0u32.to_be_bytes());
    png.extend_from_slice(b"IDAT");
    png.extend_from_slice(&0x35af061eu32.to_be_bytes());
    png.extend_from_slice(&0u32.to_be_bytes());
    png.extend_from_slice(b"IEND");
    png.extend_from_slice(&0xae426082u32.to_be_bytes());
    std::fs::write(file.path(), png).expect("write oversized PNG header");

    let error = super::prepare_extraction_input(
        file.path(),
        Some("image/png"),
        xberg::extractors::security::SecurityLimits::default().max_content_size,
    )
    .expect_err("oversized image must be rejected");
    assert!(error.to_string().contains("too large to normalize safely"));
}
