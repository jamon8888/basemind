//! Helper bodies for the `vault` domain tool — one `run_<mode>` per [`VaultMode`](super::mode::VaultMode),
//! plus the [`run_vault`] dispatcher the `#[tool]` shim and the CLI both call.
//!
//! ## Stateless design
//!
//! basemind does NOT store the vault blob. The caller (TypeScript layer) writes it to
//! `{appData}/vaults/`. This tool only encrypts/decrypts in-memory maps. `find` and `forget`
//! operate on a map supplied in the same call (never persisted here).

use std::collections::BTreeMap;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use rmcp::ErrorData as McpError;
use rmcp::model::CallToolResult;
use serde::Serialize;
use xberg::text::redaction::rehydration::{self, SubjectMatch};

use super::mode::{VaultMode, reject_unsupported};
use super::types_vault::{
    VaultDecryptResponse, VaultEncryptResponse, VaultFindResponse, VaultForgetResponse, VaultInspectResponse,
};
use crate::mcp::helpers::json_result;

fn reject_foreign_fields(mode: VaultMode, present: &[(&str, bool)], allowed: &[&str]) -> Result<(), McpError> {
    let foreign: Vec<(&str, bool)> = present
        .iter()
        .filter(|(field, _)| !allowed.contains(field))
        .copied()
        .collect();
    reject_unsupported(VaultMode::DOMAIN, mode.as_str(), &foreign)
}

fn require_field<T>(mode: VaultMode, field: &str, value: Option<T>) -> Result<T, McpError> {
    value
        .ok_or_else(|| McpError::invalid_params(format!("`vault` mode=\"{}\" requires `{field}`", mode.as_str()), None))
}

fn require_map(mode: VaultMode, map: Option<BTreeMap<String, String>>) -> Result<BTreeMap<String, String>, McpError> {
    map.ok_or_else(|| {
        McpError::invalid_params(
            format!(
                "`vault` mode=\"{}\" requires `map` (the rehydration map)",
                mode.as_str()
            ),
            None,
        )
    })
}

fn decode_blob(blob_b64: &str) -> Result<Vec<u8>, McpError> {
    BASE64
        .decode(blob_b64.as_bytes())
        .map_err(|e| McpError::invalid_params(format!("failed to base64-decode `encrypted_blob`: {e}"), None))
}

fn encode_blob(bytes: &[u8]) -> String {
    BASE64.encode(bytes)
}

fn category_from_token(token: &str) -> Option<String> {
    let inner = token.strip_prefix('[')?.strip_suffix(']')?;
    let underscore_idx = inner.rfind('_')?;
    let (category, rest) = inner.split_at(underscore_idx);
    let suffix = &rest[1..];
    if category.is_empty() || suffix.is_empty() || !suffix.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(category.to_string())
}

pub(super) async fn run_vault(p: super::types_vault::VaultParams) -> Result<CallToolResult, McpError> {
    let mode = p.mode;
    let present: Vec<(&str, bool)> = [
        ("map", p.map.is_some()),
        ("encrypted_blob", p.encrypted_blob.is_some()),
        ("passphrase", p.passphrase.is_some()),
        ("query", p.query.is_some()),
    ]
    .to_vec();

    match mode {
        VaultMode::Encrypt => {
            reject_foreign_fields(mode, &present, &["map", "passphrase"])?;
            let map = require_map(mode, p.map)?;
            let passphrase = require_field(mode, "passphrase", p.passphrase)?;
            let token_count = map.len();
            let encrypted = rehydration::encrypt_map(&map, &passphrase)
                .map_err(|e| McpError::internal_error(format!("encryption failed: {e}"), None))?;
            let response = VaultEncryptResponse {
                encrypted_blob: encode_blob(&encrypted),
                token_count,
            };
            json_result(&response)
        }
        VaultMode::Decrypt => {
            reject_foreign_fields(mode, &present, &["encrypted_blob", "passphrase"])?;
            let blob_b64 = require_field(mode, "encrypted_blob", p.encrypted_blob)?;
            let passphrase = require_field(mode, "passphrase", p.passphrase)?;
            let blob = decode_blob(&blob_b64)?;
            let map = rehydration::decrypt_map(&blob, &passphrase)
                .map_err(|e| McpError::invalid_params(format!("decryption failed: {e}"), None))?;
            let token_count = map.len();
            let response = VaultDecryptResponse { map, token_count };
            json_result(&response)
        }
        VaultMode::Find => {
            reject_foreign_fields(mode, &present, &["map", "query"])?;
            let map = require_map(mode, p.map)?;
            let query = require_field(mode, "query", p.query)?;
            let searched_token_count = map.len();
            let matches: Vec<SubjectMatch> = rehydration::find_subject(&map, &query);
            let response = VaultFindResponse {
                matches: matches
                    .into_iter()
                    .map(|m| super::types_vault::SubjectMatch {
                        token: m.token,
                        original: m.original,
                        category: m.category,
                    })
                    .collect(),
                searched_token_count,
            };
            json_result(&response)
        }
        VaultMode::Forget => {
            reject_foreign_fields(mode, &present, &["map", "query"])?;
            let mut map = require_map(mode, p.map)?;
            let query = require_field(mode, "query", p.query)?;
            let removed: Vec<SubjectMatch> = rehydration::forget_subject(&mut map, &query);
            let remaining_token_count = map.len();
            let response = VaultForgetResponse {
                removed: removed
                    .into_iter()
                    .map(|m| super::types_vault::SubjectMatch {
                        token: m.token,
                        original: m.original,
                        category: m.category,
                    })
                    .collect(),
                remaining_token_count,
            };
            json_result(&response)
        }
        VaultMode::Inspect => {
            reject_foreign_fields(mode, &present, &["map"])?;
            let map = require_map(mode, p.map)?;
            let token_count = map.len();
            let mut categories: Vec<String> = map.keys().filter_map(|t| category_from_token(t)).collect();
            categories.sort();
            categories.dedup();
            let sample: BTreeMap<String, String> = map.iter().take(10).map(|(k, v)| (k.clone(), v.clone())).collect();
            let response = VaultInspectResponse {
                token_count,
                categories,
                sample,
            };
            json_result(&response)
        }
    }
}
