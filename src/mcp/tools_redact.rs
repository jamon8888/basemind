//! The `redact_text` domain tool — redacts arbitrary text through the same
//! xberg pipeline as document extraction.
//!
//! Flow: obtain content (inline `text`, or `file_path` extracted by xberg —
//! any format incl. images via OCR) → snapshot the original content →
//! `redact_capturing_rehydration_map` (rewrites the document and captures the
//! token map) → return `{redacted_text, rehydration_map, detections}` so the
//! caller can store the map and send redacted text onward.

use rmcp::ErrorData as McpError;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{schemars, tool};
use serde::{Deserialize, Serialize};

use super::BasemindServer;
use super::helpers::record_call;
use xberg::text::redaction;
use xberg::{ExtractInput, extract};

use crate::config::{RedactionConfig, RedactionCustomPattern, RedactionCustomTerm, RedactionStrategy};

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct RedactTextParams {
    /// Text to redact. Mutually exclusive with `file_path`.
    #[serde(default)]
    pub text: String,
    /// File to extract and redact instead of `text` — any format xberg
    /// supports (PDF, Office, HTML, email, images via OCR). Filesystem path.
    #[serde(default)]
    pub file_path: Option<String>,
    /// PII categories to redact (for example `email`, `phone`). Empty = all
    /// categories supported by the engine.
    #[serde(default)]
    pub categories: Vec<String>,
    /// Redaction strategy: `token-replace` (default, reversible), `mask`,
    /// `hash`, or `drop`.
    #[serde(default)]
    pub strategy: Option<String>,
    /// Custom literal terms to redact. Each entry is `["label", "value"]`.
    #[serde(default)]
    pub custom_terms: Vec<Vec<String>>,
    /// Custom regex patterns to redact. Each entry is `["label", "regex"]`.
    #[serde(default)]
    pub custom_patterns: Vec<Vec<String>>,
}

#[rmcp::tool_router(vis = "pub(super)", router = "tool_router_redact_text")]
impl BasemindServer {
    #[tool(
        description = "Redact arbitrary text (inline `text`, or a document `file_path` extracted by xberg — PDF/Office/HTML/images via OCR). Returns redacted_text, rehydration_map (token to original), and detections (category, start, end, text).",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    pub(crate) async fn redact_text(
        &self,
        Parameters(p): Parameters<RedactTextParams>,
        _peer: rmcp::Peer<rmcp::RoleServer>,
        _meta: rmcp::model::RequestMetaObject,
    ) -> Result<CallToolResult, McpError> {
        let started = std::time::Instant::now();
        if p.text.len() > MAX_BYTES {
            let result: Result<CallToolResult, McpError> = Err(oversized_err(p.text.len()));
            record_call(&self.state, "redact_text", &serde_json::Value::Null, started, &result);
            return result;
        }
        let params_json = serde_json::to_value(&p).unwrap_or(serde_json::Value::Null);
        let result = run_redact(p).await;
        record_call(&self.state, "redact_text", &params_json, started, &result);
        result
    }
}

impl BasemindServer {
    pub(crate) async fn redact_text_cli(&self, p: RedactTextParams) -> Result<CallToolResult, McpError> {
        let started = std::time::Instant::now();
        let params_json = serde_json::to_value(&p).unwrap_or(serde_json::Value::Null);
        let result = run_redact(p).await;
        record_call(&self.state, "redact_text", &params_json, started, &result);
        result
    }
}

const MAX_BYTES: usize = 1 << 20; // 1 MiB

fn oversized_err(len: usize) -> McpError {
    McpError::internal_error(
        format!("redact_text input too large: {len} bytes (max {MAX_BYTES})"),
        None,
    )
}

async fn run_redact(args: RedactTextParams) -> Result<CallToolResult, McpError> {
    let input = match args.file_path.as_deref() {
        Some(path) => {
            if !args.text.is_empty() {
                return Err(McpError::internal_error(
                    "redact_text accepts either text or file_path, not both".to_string(),
                    None,
                ));
            }
            let meta = std::fs::metadata(path)
                .map_err(|e| McpError::internal_error(format!("cannot read {path}: {e}"), None))?;
            if !meta.is_file() {
                return Err(McpError::internal_error(
                    format!("{path} is not a regular file"),
                    None,
                ));
            }
            ExtractInput::from_uri(path.to_string())
        }
        None => {
            if args.text.len() > MAX_BYTES {
                return Err(oversized_err(args.text.len()));
            }
            if args.text.is_empty() {
                return Err(McpError::internal_error(
                    "redact_text requires non-empty text or file_path".to_string(),
                    None,
                ));
            }
            ExtractInput::from_bytes(
                args.text.into_bytes(),
                "text/plain",
                Some("input.txt".to_string()),
            )
        }
    };

    let strategy = match args.strategy.as_deref() {
        None | Some("token-replace") => RedactionStrategy::TokenReplace,
        Some("mask") => RedactionStrategy::Mask,
        Some("hash") => RedactionStrategy::Hash,
        Some("drop") => RedactionStrategy::Drop,
        Some(other) => {
            return Err(McpError::internal_error(
                format!("unknown strategy: {other} (expected token-replace|mask|hash|drop)"),
                None,
            ));
        }
    };

    let custom_terms: Vec<RedactionCustomTerm> = args
        .custom_terms
        .into_iter()
        .filter_map(|t| {
            if t.len() == 2 {
                Some(RedactionCustomTerm {
                    label: t[0].clone(),
                    value: t[1].clone(),
                    case_sensitive: false,
                })
            } else {
                None
            }
        })
        .collect();

    let custom_patterns: Vec<RedactionCustomPattern> = args
        .custom_patterns
        .into_iter()
        .filter_map(|p| {
            if p.len() == 2 {
                Some(RedactionCustomPattern {
                    label: p[0].clone(),
                    pattern: p[1].clone(),
                    case_sensitive: false,
                })
            } else {
                None
            }
        })
        .collect();

    let basemind_config = RedactionConfig {
        enabled: true,
        categories: args.categories,
        strategy,
        custom_terms,
        custom_patterns,
        ..Default::default()
    };
    let redaction_config = basemind_config
        .to_xberg()
        .ok_or_else(|| McpError::internal_error("failed to convert redaction config".to_string(), None))?;

    let extraction_config = xberg::core::config::ExtractionConfig {
        redaction: None,
        ..Default::default()
    };

    let mut extraction = extract(input, &extraction_config)
        .await
        .map_err(|e| McpError::internal_error(format!("xberg extract failed: {e}"), None))?;
    let mut doc = extraction
        .results
        .pop()
        .ok_or_else(|| McpError::internal_error("xberg returned no extracted document".to_string(), None))?;
    // Extracted content (e.g. OCR of a large image) must honor the same cap
    // as inline text — checked after extraction because only then is the
    // byte length known.
    if doc.content.len() > MAX_BYTES {
        return Err(oversized_err(doc.content.len()));
    }
    if doc.content.is_empty() {
        return Err(McpError::internal_error(
            "redact_text extracted no text from the given input".to_string(),
            None,
        ));
    }
    let original = doc.content.clone();

    let map = redaction::redact_capturing_rehydration_map(&mut doc, &redaction_config)
        .await
        .map_err(|e| McpError::internal_error(format!("xberg redaction failed: {e}"), None))?;

    let findings = doc.redaction_report.map(|report| report.findings).unwrap_or_default();
    let detections: Vec<serde_json::Value> = findings
        .iter()
        .map(|finding| {
            let category = serde_json::to_value(&finding.category)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_else(|| "unknown".to_string());
            let start = finding.start as usize;
            let end = finding.end as usize;
            serde_json::json!({
                "category": category,
                "start": start,
                "end": end,
                "text": original.get(start..end).unwrap_or("")
            })
        })
        .collect();

    let rehydration_map: std::collections::BTreeMap<String, String> = map.into_iter().collect();

    Ok(CallToolResult::structured(serde_json::json!({
        "redacted_text": doc.content,
        "rehydration_map": rehydration_map,
        "detections": detections
    })))
}

#[cfg(test)]
#[path = "redact_file_tests.rs"]
mod redact_file_tests;
