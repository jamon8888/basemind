//! `redact_text` input-source tests: inline `text` keeps working, and the
//! new `file_path` routes through xberg extraction (any format) instead of
//! assuming UTF-8.

use rmcp::model::{CallToolResult, ContentBlock};
use serde_json::Value;

use super::{RedactTextParams, run_redact};

/// The JSON payload a tool returns (the first text content block).
fn json_of(result: &CallToolResult) -> Value {
    for content in &result.content {
        if let ContentBlock::Text(text) = content {
            return serde_json::from_str(&text.text).expect("tool payload is JSON");
        }
    }
    panic!("tool returned no text content");
}

/// Inline-text params with every option at its default (no NER, not required).
fn text_params(text: &str) -> RedactTextParams {
    RedactTextParams {
        text: text.to_string(),
        file_path: None,
        categories: vec![],
        strategy: None,
        custom_terms: vec![],
        custom_patterns: vec![],
        ner_model_dir: None,
        require_ner: false,
    }
}

/// A `ner_model_dir` pointing at nothing must never fail the call:
/// NER degrades to pattern-only redaction and the response stays intact.
#[tokio::test]
async fn ner_model_dir_missing_model_still_redacts() {
    let result = run_redact(RedactTextParams {
        ner_model_dir: Some("/nonexistent/gliner2".to_string()),
        ..text_params("Reach me at alice@example.com today")
    })
    .await
    .expect("missing NER model degrades, never errors");
    let payload = json_of(&result);
    let redacted = payload["redacted_text"].as_str().expect("redacted_text");
    assert!(
        !redacted.contains("alice@example.com"),
        "PII must still be redacted: {redacted}"
    );
    assert!(!payload["detections"].as_array().expect("detections").is_empty());
}

/// Callers that must not send anything unredacted need to tell "NER found
/// nothing" from "NER never ran"; the payload says which.
#[tokio::test]
async fn ner_ran_is_false_when_the_model_is_unavailable() {
    for dir in [None, Some("/nonexistent/gliner2".to_string())] {
        let result = run_redact(RedactTextParams {
            ner_model_dir: dir,
            ..text_params("Reach me at alice@example.com today")
        })
        .await
        .expect("degrades to pattern-only by default");
        assert_eq!(json_of(&result)["ner_ran"], Value::Bool(false));
    }
}

/// With `require_ner`, an unavailable model is an error, never a silent
/// pattern-only result.
#[tokio::test]
async fn require_ner_fails_instead_of_degrading() {
    for dir in [None, Some("/nonexistent/gliner2".to_string())] {
        let error = run_redact(RedactTextParams {
            ner_model_dir: dir,
            require_ner: true,
            ..text_params("Alice Smith owes 10 000 EUR")
        })
        .await
        .expect_err("require_ner must not fall back to pattern-only redaction");
        assert!(
            error.message.contains("NER required"),
            "error names the cause: {}",
            error.message
        );
    }
}

#[tokio::test]
async fn text_param_still_redacts() {
    let result = run_redact(text_params("Reach me at alice@example.com today"))
        .await
        .expect("redact_text succeeds");
    let payload = json_of(&result);
    let redacted = payload["redacted_text"].as_str().expect("redacted_text");
    assert!(
        !redacted.contains("alice@example.com"),
        "PII must be redacted: {redacted}"
    );
    assert!(
        !payload["rehydration_map"].as_object().expect("map").is_empty(),
        "rehydration_map must capture the original"
    );
    assert!(!payload["detections"].as_array().expect("detections").is_empty());
}

#[tokio::test]
async fn file_path_redacts_extracted_text() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("note.txt");
    std::fs::write(&file, "Contact bob@corp.com for details").expect("write fixture");

    let result = run_redact(RedactTextParams {
        text: String::new(),
        file_path: Some(file.to_string_lossy().into_owned()),
        categories: vec![],
        strategy: None,
        custom_terms: vec![],
        custom_patterns: vec![],
        ner_model_dir: None,
        require_ner: false,
    })
    .await
    .expect("redact_text succeeds");
    let payload = json_of(&result);
    let redacted = payload["redacted_text"].as_str().expect("redacted_text");
    assert!(!redacted.contains("bob@corp.com"), "PII must be redacted: {redacted}");
    assert!(!payload["detections"].as_array().expect("detections").is_empty());
}

#[tokio::test]
async fn file_path_missing_file_errors() {
    let dir = tempfile::tempdir().expect("tempdir");
    let missing = dir.path().join("nope.txt");
    let err = run_redact(RedactTextParams {
        text: String::new(),
        file_path: Some(missing.to_string_lossy().into_owned()),
        categories: vec![],
        strategy: None,
        custom_terms: vec![],
        custom_patterns: vec![],
        ner_model_dir: None,
        require_ner: false,
    })
    .await
    .expect_err("missing file must fail");
    assert!(err.message.contains("cannot read"), "unexpected error: {}", err.message);
}

#[tokio::test]
async fn text_and_file_path_together_rejected() {
    let err = run_redact(RedactTextParams {
        text: "hello".to_string(),
        file_path: Some("/tmp/whatever.txt".to_string()),
        categories: vec![],
        strategy: None,
        custom_terms: vec![],
        custom_patterns: vec![],
        ner_model_dir: None,
        require_ner: false,
    })
    .await
    .expect_err("both inputs must fail");
    assert!(err.message.contains("not both"), "unexpected error: {}", err.message);
}

#[tokio::test]
async fn oversized_extracted_content_errors() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("big.txt");
    std::fs::write(&file, "x".repeat(2 << 20)).expect("write fixture");

    let err = run_redact(RedactTextParams {
        text: String::new(),
        file_path: Some(file.to_string_lossy().into_owned()),
        categories: vec![],
        strategy: None,
        custom_terms: vec![],
        custom_patterns: vec![],
        ner_model_dir: None,
        require_ner: false,
    })
    .await
    .expect_err("oversized extraction must fail");
    assert!(err.message.contains("too large"), "unexpected error: {}", err.message);
}

/// A raw file over the pre-extraction cap is rejected before xberg runs any
/// PDF/image/OCR work — a sparse file proves the guard fires on the raw
/// size alone, without paying for 64 MiB of content.
#[tokio::test]
async fn raw_file_size_capped_before_extraction() {
    let dir = tempfile::tempdir().expect("tempdir");
    let file = dir.path().join("huge.bin");
    std::fs::File::create(&file)
        .expect("create fixture")
        .set_len(super::MAX_FILE_BYTES + 1)
        .expect("extend to sparse size");

    let err = run_redact(RedactTextParams {
        text: String::new(),
        file_path: Some(file.to_string_lossy().into_owned()),
        ..text_params("")
    })
    .await
    .expect_err("oversized raw file must fail before extraction");
    assert!(err.message.contains("too large"), "unexpected error: {}", err.message);
}

/// NER-derived labels (`person`, `location`, …) surface as plain string
/// categories in detections — never the `{"custom": …}` object the serde
/// default renders, which the app-side parser would report as "unknown".
#[tokio::test]
async fn custom_term_labels_surface_in_detections() {
    let result = run_redact(RedactTextParams {
        text: "Alice visited Paris today".to_string(),
        custom_terms: vec![
            vec!["person".to_string(), "Alice".to_string()],
            vec!["location".to_string(), "Paris".to_string()],
        ],
        ..text_params("Alice visited Paris today")
    })
    .await
    .expect("redact_text succeeds");
    let payload = json_of(&result);
    let detections = payload["detections"].as_array().expect("detections");
    let categories: Vec<&str> = detections
        .iter()
        .map(|d| d["category"].as_str().unwrap_or("unknown"))
        .collect();
    assert!(
        categories.contains(&"person"),
        "person must surface as a plain string: {categories:?}"
    );
    assert!(
        categories.contains(&"location"),
        "location must surface as a plain string: {categories:?}"
    );
    assert!(
        !categories.contains(&"unknown"),
        "no detection may read unknown: {categories:?}"
    );
    let redacted = payload["redacted_text"].as_str().expect("redacted_text");
    assert!(redacted.contains("[PERSON_1]"), "person token missing: {redacted}");
    assert!(redacted.contains("[LOCATION_1]"), "location token missing: {redacted}");
}

/// Overlapping NER spans collapse to the earliest, longest mention — one
/// custom term, one redaction token, even when categories disagree.
#[test]
fn overlapping_ner_spans_dedup_to_one_mention() {
    use xberg::types::entity::{Entity, EntityCategory};
    let entities = vec![
        Entity {
            category: EntityCategory::Person,
            text: "Alice Smith".to_string(),
            start: 5,
            end: 16,
            confidence: Some(0.97),
        },
        Entity {
            category: EntityCategory::Location,
            text: "Smith".to_string(),
            start: 11,
            end: 16,
            confidence: Some(0.6),
        },
        Entity {
            category: EntityCategory::Organization,
            text: "Acme".to_string(),
            start: 20,
            end: 24,
            confidence: Some(0.9),
        },
    ];
    let kept = super::dedup_overlapping(entities);
    assert_eq!(kept.len(), 2, "overlapping span must be dropped");
    assert_eq!(kept[0].text, "Alice Smith");
    assert_eq!(kept[0].confidence, Some(0.97));
    assert_eq!(kept[1].text, "Acme");
}

/// NER spans redact whole words only: a short span like "US" must never
/// corrupt unrelated words ("because"), and regex metacharacters in a span
/// must be escaped, not interpreted.
#[test]
fn ner_span_patterns_are_boundary_aware_and_escaped() {
    let us = regex::Regex::new(&super::ner_span_pattern("US")).expect("valid regex");
    assert!(us.is_match("US policy"));
    assert!(!us.is_match("because"), "span must not match inside a word");

    let dotted = regex::Regex::new(&super::ner_span_pattern("Acme Corp.")).expect("metacharacters must be escaped");
    assert!(dotted.is_match("at Acme Corp. HQ"));
    assert!(
        dotted.find("Acme CorpX HQ").is_none(),
        "literal dot must not match any char"
    );

    let tagged = regex::Regex::new(&super::ner_span_pattern("#42")).expect("valid regex");
    assert!(tagged.is_match("issue #42 today"), "leading punctuation needs no \\b");
    assert!(
        !tagged.is_match("issue #425"),
        "trailing \\b still bounds the word edge"
    );
}

/// Download-gated (#37): set `BASEMIND_NER_MODEL_DIR` to the staged GLiNER2
/// directory to verify real detection — honest categories plus confidence.
/// Uses the multi-thread runtime because the candle backend calls
/// `tokio::task::block_in_place` during inference.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the staged GLiNER2 model; set BASEMIND_NER_MODEL_DIR"]
async fn ner_model_dir_detects_entities_with_confidence() {
    let dir = std::env::var("BASEMIND_NER_MODEL_DIR").expect("set BASEMIND_NER_MODEL_DIR to run this ignored test");
    let result = run_redact(RedactTextParams {
        text: "Alice Smith met Bob Jones in Paris near Acme Corp.".to_string(),
        ner_model_dir: Some(dir),
        ..text_params("Alice Smith met Bob Jones in Paris near Acme Corp.")
    })
    .await
    .expect("redact_text succeeds with a real model");
    let payload = json_of(&result);
    let detections = payload["detections"].as_array().expect("detections");
    let categories: Vec<&str> = detections
        .iter()
        .map(|d| d["category"].as_str().unwrap_or("unknown"))
        .collect();
    // The 42-label list makes GLiNER2 pick the most specific label it was
    // given (`full_name`, `city`, …) rather than the generic `person` /
    // `location`, so assert on the families.
    const PERSON_LABELS: &[&str] = &["person", "full_name", "first_name", "last_name"];
    const LOCATION_LABELS: &[&str] = &[
        "location",
        "city",
        "address",
        "street_address",
        "country",
        "state_or_region",
    ];
    assert!(
        categories.iter().any(|c| PERSON_LABELS.contains(c)),
        "a person label must be detected: {categories:?}"
    );
    assert!(
        categories.iter().any(|c| LOCATION_LABELS.contains(c)),
        "a location label must be detected: {categories:?}"
    );
    assert!(
        !categories.contains(&"unknown"),
        "no detection may read unknown: {categories:?}"
    );
    assert!(
        detections
            .iter()
            .any(|d| d.get("confidence").and_then(|c| c.as_f64()).is_some()),
        "NER spans must carry confidence"
    );
    assert_eq!(payload["ner_ran"], Value::Bool(true));
    let redacted = payload["redacted_text"].as_str().expect("redacted_text");
    assert!(!redacted.contains("Alice Smith"), "person must be redacted: {redacted}");
}

/// xberg's built-in phone pattern misses these formats (a `+` after a space
/// defeats its leading `\b`; French numbers are written in digit pairs).
#[tokio::test]
async fn international_and_french_phone_formats_redact() {
    for phone in [
        "+33 6 12 34 56 78",
        "+33612345678",
        "06 12 34 56 78",
        "06.12.34.56.78",
        "+49 30 1234567",
        "+44 20 7946 0958",
        "+33 (0)1 42 68 53 00",
    ] {
        let text = format!("Appelez le {phone} demain");
        let result = run_redact(text_params(&text)).await.expect("redact_text succeeds");
        let payload = json_of(&result);
        let redacted = payload["redacted_text"].as_str().expect("redacted_text");
        assert!(!redacted.contains(phone), "phone {phone} must be redacted: {redacted}");
        let map = payload["rehydration_map"].as_object().expect("map");
        assert!(
            map.values().any(|v| v.as_str() == Some(phone)),
            "rehydration map must hold {phone} verbatim: {map:?}"
        );
    }
}

/// The phone patterns must not swallow dates, amounts or short numbers.
#[tokio::test]
async fn phone_patterns_leave_dates_and_amounts_alone() {
    let text = "Réunion le 28/09/2026 à 14:30, montant 1 234,56 €, dossier 2026-09-28, page 12";
    let result = run_redact(text_params(text)).await.expect("redact_text succeeds");
    let payload = json_of(&result);
    let redacted = payload["redacted_text"].as_str().expect("redacted_text");
    for kept in ["28/09/2026", "14:30", "1 234,56", "2026-09-28", "page 12"] {
        assert!(redacted.contains(kept), "{kept} must stay: {redacted}");
    }
}

/// The AI Act citation pattern matches the regulation's own number and
/// nothing else: prose that merely names the act, and every other EU
/// regulation, must survive untouched.
#[tokio::test]
async fn ai_act_citation_redacts_only_the_2024_1689_number() {
    let text = "Per the AI Act, Regulation (EU) 2024/1689 applies; see also Regulation (EU) 2022/2065.";
    let result = run_redact(text_params(text)).await.expect("redact_text succeeds");
    let payload = json_of(&result);
    let redacted = payload["redacted_text"].as_str().expect("redacted_text");
    assert!(
        !redacted.contains("2024/1689"),
        "AI Act citation must be redacted: {redacted}"
    );
    assert!(
        redacted.contains("the AI Act"),
        "prose naming the act must stay: {redacted}"
    );
    assert!(
        redacted.contains("2022/2065"),
        "other regulations must stay: {redacted}"
    );
    let detections = payload["detections"].as_array().expect("detections");
    assert!(
        detections
            .iter()
            .any(|detection| detection["category"] == "ai_act_citation"),
        "the citation must surface as ai_act_citation: {detections:?}"
    );
}

// Offsets index the text the caller sent (#70). Inline text used to go through
// xberg's plain-text extractor, which trims every paragraph, collapses blank-line
// runs, turns CRLF into LF and folds decomposed accents, then reported offsets on
// that rewritten text. A caller mapping spans back onto its own string was off by
// the amount the extractor removed.

/// Redacts `text` with no NER and returns the detections whose text is `needle`,
/// asserting each one indexes `text` itself (UTF-8 bytes), not an extracted copy.
async fn assert_spans_index_the_input(text: &str, needle: &str) -> Value {
    let result = run_redact(text_params(text)).await.expect("redact_text succeeds");
    let payload = json_of(&result);
    let detections = payload["detections"].as_array().expect("detections");
    let found: Vec<&Value> = detections
        .iter()
        .filter(|d| d["text"].as_str().is_some_and(|t| t.contains(needle)))
        .collect();
    assert!(
        !found.is_empty(),
        "no detection covers {needle:?} in {text:?}: {detections:?}"
    );
    for detection in found {
        let start = detection["start"].as_u64().expect("start") as usize;
        let end = detection["end"].as_u64().expect("end") as usize;
        let slice = text.as_bytes().get(start..end).unwrap_or_default();
        assert_eq!(
            std::str::from_utf8(slice).unwrap_or("<not on a char boundary>"),
            detection["text"].as_str().expect("detection text"),
            "[{start},{end}) does not index the input {text:?}"
        );
    }
    payload
}

#[tokio::test]
async fn offsets_hold_with_leading_whitespace() {
    for text in [
        " Contact: jane.roe@exemple.fr merci",
        "   Contact: jane.roe@exemple.fr merci",
        "\n\nContact: jane.roe@exemple.fr merci",
    ] {
        assert_spans_index_the_input(text, "jane.roe@exemple.fr").await;
    }
}

#[tokio::test]
async fn offsets_hold_after_blank_line_runs_and_indented_paragraphs() {
    for text in [
        "Intro\n\n\n\nContact: jane.roe@exemple.fr merci",
        "Intro\n\n   Contact: jane.roe@exemple.fr merci",
        "Intro   \n\nContact: jane.roe@exemple.fr merci",
    ] {
        assert_spans_index_the_input(text, "jane.roe@exemple.fr").await;
    }
}

#[tokio::test]
async fn offsets_hold_with_windows_line_endings() {
    assert_spans_index_the_input("Intro\r\nContact: jane.roe@exemple.fr\r\nmerci", "jane.roe@exemple.fr").await;
}

#[tokio::test]
async fn offsets_hold_after_decomposed_accents() {
    // "Hélène" written as e + combining accent: 2 bytes per mark.
    let text = "Prénom H\u{65}\u{301}l\u{65}\u{300}ne Dubreuil écrit: jane.roe@exemple.fr";
    assert_spans_index_the_input(text, "jane.roe@exemple.fr").await;
}

#[tokio::test]
async fn redacted_text_keeps_the_surrounding_text_verbatim() {
    let text = "  Intro\r\n\r\n\r\nContact: jane.roe@exemple.fr merci\n\n";
    let result = run_redact(text_params(text)).await.expect("redact_text succeeds");
    let redacted = json_of(&result)["redacted_text"]
        .as_str()
        .expect("redacted_text")
        .to_string();
    assert!(redacted.starts_with("  Intro\r\n\r\n\r\nContact: "), "{redacted:?}");
    assert!(redacted.ends_with(" merci\n\n"), "{redacted:?}");
    assert!(!redacted.contains("jane.roe@exemple.fr"), "{redacted:?}");
}
