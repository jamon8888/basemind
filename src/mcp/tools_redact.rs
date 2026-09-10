//! The `redact_text` domain tool — redacts arbitrary text through the same
//! xberg pipeline as document extraction.
//!
//! Flow: extract bytes as plain text (no redaction) → snapshot the original
//! content → `redact_capturing_rehydration_map` (rewrites the document and
//! captures the token map) → return `{redacted_text, rehydration_map,
//! detections}` so the caller can store the map and send redacted text onward.

use rmcp::ErrorData as McpError;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{schemars, tool};
use serde::{Deserialize, Serialize};

use super::BasemindServer;
use super::helpers::record_call;
use xberg::text::redaction;
use xberg::{ExtractInput, extract};

use crate::config::{RedactionConfig, RedactionStrategy};

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct RedactTextParams {
    /// Text to redact.
    #[serde(default)]
    pub text: String,
    /// PII categories to redact (for example `email`, `phone`). Empty = all
    /// categories supported by the engine.
    #[serde(default)]
    pub categories: Vec<String>,
}

#[rmcp::tool_router(vis = "pub(super)", router = "tool_router_redact_text")]
impl BasemindServer {
    #[tool(
        description = "Redact arbitrary text using the same pipeline as document extraction. Returns redacted_text, rehydration_map (token to original), and detections (category, start, end, text).",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
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
        let params_json = serde_json::to_value(&p).unwrap_or(serde_json::Value::Null);
        let result = run_redact(p).await;
        record_call(&self.state, "redact_text", &params_json, started, &result);
        result
    }
}

async fn run_redact(args: RedactTextParams) -> Result<CallToolResult, McpError> {
    if args.text.is_empty() {
        return Err(McpError::internal_error(
            "redact_text requires non-empty text".to_string(),
            None,
        ));
    }

    let basemind_config = RedactionConfig {
        enabled: true,
        categories: args.categories,
        strategy: RedactionStrategy::TokenReplace,
        ..Default::default()
    };
    let redaction_config = basemind_config
        .to_xberg()
        .ok_or_else(|| McpError::internal_error("failed to convert redaction config".to_string(), None))?;

    let mut extraction_config = xberg::core::config::ExtractionConfig::default();
    extraction_config.redaction = None;

    let input = ExtractInput::from_bytes(args.text.into_bytes(), "text/plain", Some("input.txt".to_string()));
    let mut extraction = extract(input, &extraction_config)
        .await
        .map_err(|e| McpError::internal_error(format!("xberg extract failed: {e}"), None))?;
    let mut doc = extraction
        .results
        .pop()
        .ok_or_else(|| McpError::internal_error("xberg returned no extracted document".to_string(), None))?;
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
