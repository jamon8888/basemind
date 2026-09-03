//! The `vault` domain tool shim for `BasemindServer`.
//!
//! One tool, one required `mode` — encrypt, decrypt, find, forget, inspect — dispatched to
//! `helpers_vault::run_vault`. Thin wrapper: the bodies live in `helpers_vault.rs`.

use rmcp::ErrorData as McpError;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::tool;
use serde_json::Value;

use super::BasemindServer;
use super::helpers::record_call;
use super::lenient::Lenient;
use super::types_vault::VaultParams;

#[rmcp::tool_router(vis = "pub(super)", router = "tool_router_vault")]
impl BasemindServer {
    #[tool(
        description = "Manage encrypted PII rehydration maps produced by document ingestion with \
        `redaction.enabled = true`. basemind does NOT store the vault blob — the caller (TypeScript \
        layer) persists it to `{appData}/vaults/`. This tool only encrypts, decrypts, searches, and \
        modifies maps in memory. `mode` is required. `encrypt` takes a rehydration `map` \
        (token→original, returned by the extraction result's `rehydration_map`) and a `passphrase`, \
        returning a base64 `encrypted_blob` (XPII wire format: magic + scrypt-derived AES-256-GCM). \
        `decrypt` reverses `encrypt`: takes `encrypted_blob` + `passphrase`, returns the `map`. \
        `find` searches a decrypted `map` for `query` — exact token match or case-insensitive \
        substring on the original value — returning all matches sorted by token. `forget` removes \
        every entry matching `query` from the in-memory `map` and returns the removed entries \
        (the caller re-encrypts and persists the trimmed map). `inspect` returns token count, \
        sorted deduped category list, and up to 10 sample entries from a decrypted `map`. \
        Parameters that belong to another mode are rejected, not ignored.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    pub(crate) async fn vault(
        &self,
        Parameters(Lenient(p)): Parameters<Lenient<VaultParams>>,
        _peer: rmcp::Peer<rmcp::RoleServer>,
        _meta: rmcp::model::RequestMetaObject,
    ) -> Result<CallToolResult, McpError> {
        let started = std::time::Instant::now();
        let key = p.mode.telemetry_key();
        let params_json = serde_json::to_value(&p).unwrap_or(Value::Null);
        let result = super::helpers_vault::run_vault(p).await;
        record_call(&self.state, key, &params_json, started, &result);
        result
    }
}

impl BasemindServer {
    pub(crate) async fn vault_cli(&self, p: VaultParams) -> Result<CallToolResult, McpError> {
        let started = std::time::Instant::now();
        let key = p.mode.telemetry_key();
        let params_json = serde_json::to_value(&p).unwrap_or(Value::Null);
        let result = super::helpers_vault::run_vault(p).await;
        record_call(&self.state, key, &params_json, started, &result);
        result
    }
}
