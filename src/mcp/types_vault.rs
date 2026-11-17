//! Request + response shapes for the `vault` domain tool.
//!
//! [`VaultParams`] is what crosses the wire: one flat parameter object with a required
//! [`VaultMode`] selecting the operation and every per-mode field as an optional sibling.
//!
//! Split out of `types.rs` to keep that file under the 1000-line cap.

use rmcp::schemars;
use serde::{Deserialize, Serialize};

use super::mode::VaultMode;

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct VaultParams {
    pub mode: VaultMode,
    #[serde(default)]
    pub map: Option<std::collections::BTreeMap<String, String>>,
    #[serde(default)]
    pub encrypted_blob: Option<String>,
    #[serde(default)]
    pub passphrase: Option<String>,
    #[serde(default)]
    pub query: Option<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct SubjectMatch {
    pub token: String,
    pub original: String,
    pub category: Option<String>,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct VaultEncryptResponse {
    pub encrypted_blob: String,
    pub token_count: usize,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct VaultDecryptResponse {
    pub map: std::collections::BTreeMap<String, String>,
    pub token_count: usize,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct VaultFindResponse {
    pub matches: Vec<SubjectMatch>,
    pub searched_token_count: usize,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct VaultForgetResponse {
    pub removed: Vec<SubjectMatch>,
    pub remaining_token_count: usize,
}

#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct VaultInspectResponse {
    pub token_count: usize,
    pub categories: Vec<String>,
    pub sample: std::collections::BTreeMap<String, String>,
}
