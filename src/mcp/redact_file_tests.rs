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
    })
    .await
    .expect_err("oversized extraction must fail");
    assert!(err.message.contains("too large"), "unexpected error: {}", err.message);
}
