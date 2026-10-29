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
    let x = cfg.to_xberg();
    assert!(x.keywords.is_some(), "Full keeps keyword extraction");
    assert!(x.ner.is_some(), "Full keeps NER");
    assert!(x.summarization.is_some(), "Full keeps summarisation");
    assert!(!x.disable_ocr, "Full leaves OCR enabled");
    assert!(x.chunking.as_ref().and_then(|c| c.embedding.as_ref()).is_none());
}

#[test]
fn to_xberg_code_only_strips_enrichment_disables_ocr_keeps_embeddings() {
    let cfg = doc_config_all_enrichment_on(DocumentModelProfile::CodeOnly);
    let x = cfg.to_xberg();
    assert!(x.keywords.is_none(), "CodeOnly forces keywords off");
    assert!(x.ner.is_none(), "CodeOnly forces NER off");
    assert!(x.summarization.is_none(), "CodeOnly forces summarisation off");
    assert!(x.disable_ocr, "CodeOnly disables OCR");
    assert!(x.chunking.as_ref().and_then(|c| c.embedding.as_ref()).is_none());
}

#[test]
fn to_xberg_none_profile_strips_everything_including_embeddings() {
    let cfg = doc_config_all_enrichment_on(DocumentModelProfile::None_);
    let x = cfg.to_xberg();
    assert!(x.keywords.is_none(), "None strips keywords");
    assert!(x.ner.is_none(), "None strips NER");
    assert!(x.summarization.is_none(), "None strips summarisation");
    assert!(x.disable_ocr, "None disables OCR");
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
    let x = cfg.to_xberg();
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

    let xberg = cfg.to_xberg();
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
