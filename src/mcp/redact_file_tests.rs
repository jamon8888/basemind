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

fn text_params(text: &str) -> RedactTextParams {
    RedactTextParams {
        text: text.to_string(),
        file_path: None,
        categories: vec![],
        strategy: None,
        custom_terms: vec![],
        custom_patterns: vec![],
        ner_model_dir: None,
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
    })
    .await
    .expect_err("oversized extraction must fail");
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
    assert!(
        categories.contains(&"person"),
        "person must be detected: {categories:?}"
    );
    assert!(
        categories.contains(&"location"),
        "location must be detected: {categories:?}"
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
    let redacted = payload["redacted_text"].as_str().expect("redacted_text");
    assert!(!redacted.contains("Alice Smith"), "person must be redacted: {redacted}");
}
