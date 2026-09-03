//! CLI parity for the `vault` tool: encrypt, decrypt, find, forget, inspect.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

use super::render::Emit;
use crate::cli::context::BasemindServer;
use crate::mcp::mode::VaultMode;
use crate::mcp::types_vault::VaultParams;

#[derive(Parser, Debug)]
pub enum VaultCmd {
    Encrypt {
        /// JSON file containing the rehydration map. Omit or pass `-` to read from stdin.
        #[arg(long, value_name = "FILE")]
        map: Option<PathBuf>,
        /// Passphrase for AES-256-GCM encryption.
        #[arg(long, value_name = "PASSPHRASE")]
        passphrase: String,
    },
    Decrypt {
        /// Base64-encoded encrypted vault blob.
        #[arg(long, value_name = "BLOB")]
        encrypted_blob: String,
        /// Passphrase for AES-256-GCM decryption.
        #[arg(long, value_name = "PASSPHRASE")]
        passphrase: String,
    },
    Find {
        /// JSON file containing the rehydration map. Omit or pass `-` to read from stdin.
        #[arg(long, value_name = "FILE")]
        map: Option<PathBuf>,
        /// Query string: exact token match or case-insensitive substring on original value.
        #[arg(long, value_name = "QUERY")]
        query: String,
    },
    Forget {
        /// JSON file containing the rehydration map. Omit or pass `-` to read from stdin.
        #[arg(long, value_name = "FILE")]
        map: Option<PathBuf>,
        /// Query string: exact token match or case-insensitive substring on original value.
        #[arg(long, value_name = "QUERY")]
        query: String,
    },
    Inspect {
        /// JSON file containing the rehydration map. Omit or pass `-` to read from stdin.
        #[arg(long, value_name = "FILE")]
        map: Option<PathBuf>,
    },
}

fn read_map(path: Option<&PathBuf>) -> Result<BTreeMap<String, String>> {
    let json = match path {
        Some(p) if p.as_os_str() != "-" => {
            std::fs::read_to_string(p).with_context(|| format!("read {}", p.display()))?
        }
        _ => {
            let mut raw = Vec::new();
            std::io::stdin().read_to_end(&mut raw).context("read stdin")?;
            String::from_utf8_lossy(&raw).into_owned()
        }
    };
    let map: BTreeMap<String, String> = serde_json::from_str(&json).context("parse rehydration map as JSON object")?;
    Ok(map)
}

pub async fn run(server: &BasemindServer, cmd: VaultCmd, _opts: &Emit, _out: &mut impl Write) -> Result<()> {
    let params = match cmd {
        VaultCmd::Encrypt { map, passphrase } => VaultParams {
            mode: VaultMode::Encrypt,
            map: Some(read_map(map.as_ref())?),
            encrypted_blob: None,
            passphrase: Some(passphrase),
            query: None,
        },
        VaultCmd::Decrypt {
            encrypted_blob,
            passphrase,
        } => VaultParams {
            mode: VaultMode::Decrypt,
            map: None,
            encrypted_blob: Some(encrypted_blob),
            passphrase: Some(passphrase),
            query: None,
        },
        VaultCmd::Find { map, query } => VaultParams {
            mode: VaultMode::Find,
            map: Some(read_map(map.as_ref())?),
            encrypted_blob: None,
            passphrase: None,
            query: Some(query),
        },
        VaultCmd::Forget { map, query } => VaultParams {
            mode: VaultMode::Forget,
            map: Some(read_map(map.as_ref())?),
            encrypted_blob: None,
            passphrase: None,
            query: Some(query),
        },
        VaultCmd::Inspect { map } => VaultParams {
            mode: VaultMode::Inspect,
            map: Some(read_map(map.as_ref())?),
            encrypted_blob: None,
            passphrase: None,
            query: None,
        },
    };
    let result = server.vault_cli(params).await?;
    let json = serde_json::to_string_pretty(&serde_json::to_value(result)?)?;
    println!("{json}");
    Ok(())
}
