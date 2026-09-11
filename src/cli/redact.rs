//! `basemind redact` — PII detection and redaction for arbitrary text.
//!
//! Dispatches through the same `redact_text` MCP tool method as the server,
//! ensuring CLI-MCP parity.

use std::io::Write;

use anyhow::{Context, Result};
use clap::Args;

use crate::cli::render;
use crate::mcp::BasemindServer;
use crate::mcp::tools_redact::{CustomPatternParam, CustomTermParam, RedactTextParams};

#[derive(Args, Debug)]
pub struct RedactArgs {
    /// Text to redact. Mutually exclusive with --file.
    #[arg(long, conflicts_with = "file")]
    pub text: Option<String>,

    /// File to read and redact. Mutually exclusive with --text. Use `-` for stdin.
    #[arg(long, conflicts_with = "text", value_name = "FILE")]
    pub file: Option<String>,

    /// Redaction strategy: token-replace (default) | mask | hash | drop.
    #[arg(long, default_value = "token-replace", value_name = "STRATEGY")]
    pub strategy: String,

    /// Comma-separated list of PII categories to redact.
    #[arg(long, value_delimiter = ',', value_name = "CATEGORIES")]
    pub categories: Vec<String>,

    /// Custom literal terms to redact. Format: `--custom-term label,value`.
    #[arg(long = "custom-term", value_name = "LABEL,VALUE")]
    pub custom_terms: Vec<String>,

    /// Custom regex patterns to redact. Format: `--custom-pattern label,regex`.
    #[arg(long = "custom-pattern", value_name = "LABEL,REGEX")]
    pub custom_patterns: Vec<String>,

    /// Output machine-readable JSON regardless of TTY.
    #[arg(long)]
    pub json: bool,
}

fn parse_custom_terms(raw: Vec<String>) -> Result<Vec<CustomTermParam>> {
    raw.into_iter()
        .map(|s| {
            let parts: Vec<&str> = s.splitn(2, ',').collect();
            anyhow::ensure!(parts.len() == 2, "--custom-term must be label,value (got: {s})");
            Ok(CustomTermParam {
                label: parts[0].to_string(),
                value: parts[1].to_string(),
            })
        })
        .collect()
}

fn parse_custom_patterns(raw: Vec<String>) -> Result<Vec<CustomPatternParam>> {
    raw.into_iter()
        .map(|s| {
            let parts: Vec<&str> = s.splitn(2, ',').collect();
            anyhow::ensure!(parts.len() == 2, "--custom-pattern must be label,regex (got: {s})");
            Ok(CustomPatternParam {
                label: parts[0].to_string(),
                pattern: parts[1].to_string(),
            })
        })
        .collect()
}

fn read_input(args: &RedactArgs) -> Result<String> {
    match (&args.text, &args.file) {
        (Some(t), None) => Ok(t.clone()),
        (None, Some(f)) if f == "-" => {
            let mut buf = Vec::new();
            std::io::stdin().read_to_end(&mut buf).context("read stdin")?;
            Ok(String::from_utf8_lossy(&buf).into_owned())
        }
        (None, Some(path)) => std::fs::read_to_string(path).with_context(|| format!("read {}", path)),
        _ => anyhow::bail!("exactly one of --text or --file is required"),
    }
}

pub async fn run(server: &BasemindServer, args: &RedactArgs, out: &mut impl Write) -> Result<()> {
    let text = read_input(args)?;
    let params = RedactTextParams {
        text,
        categories: args.categories.clone(),
        strategy: Some(args.strategy.clone()),
        custom_terms: parse_custom_terms(args.custom_terms.clone())?,
        custom_patterns: parse_custom_patterns(args.custom_patterns.clone())?,
    };

    let result = server
        .redact_text_cli(params)
        .await
        .map_err(|e| anyhow::anyhow!("redact_text: {e}"))?;

    let value = render::result_to_value(&result)?;
    if args.json {
        serde_json::to_writer(out, &value)?;
        writeln!(out)?;
    } else {
        render::render_human("redact_text", &value, out)?;
    }
    Ok(())
}
