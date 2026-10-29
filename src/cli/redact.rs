//! `basemind redact` — PII detection and redaction for arbitrary text.
//!
//! Dispatches through the same `redact_text` MCP tool method as the server,
//! ensuring CLI-MCP parity.

use std::io::Read;
use std::io::Write;

use anyhow::{Context, Result};
use clap::Args;

use crate::cli::render;
use crate::mcp::BasemindServer;
use crate::mcp::params::RedactTextParams;

#[derive(Args, Debug)]
pub struct RedactArgs {
    /// Text to redact. Mutually exclusive with --file.
    #[arg(long, conflicts_with = "file")]
    pub text: Option<String>,

    /// File to extract and redact (any xberg-supported format, incl. images via OCR). Mutually exclusive with --text. Use `-` for stdin.
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

    /// Case dictionary entry: the value that must not reach the agent, and the code name that
    /// stands in for it. Format: `--pseudonym value,code-name`. Repeatable.
    #[arg(long = "pseudonym", value_name = "VALUE,CODE_NAME")]
    pub pseudonyms: Vec<String>,

    /// Local GLiNER2 model directory. A missing dir degrades to pattern-only redaction.
    #[arg(long, value_name = "DIR")]
    pub ner_model_dir: Option<String>,

    /// Named, versioned NER label profile (full@1 or legal-fr@1).
    #[arg(long, value_name = "PROFILE")]
    pub ner_profile: Option<String>,

    /// Output machine-readable JSON regardless of TTY.
    #[arg(long)]
    pub json: bool,
}

fn parse_pairs(raw: Vec<String>) -> Result<Vec<Vec<String>>> {
    raw.into_iter()
        .map(|s| {
            let parts: Vec<&str> = s.splitn(2, ',').collect();
            anyhow::ensure!(parts.len() == 2, "expected label,value (got: {s})");
            Ok(parts.into_iter().map(str::to_string).collect())
        })
        .collect()
}

/// Returns `(text, file_path)`: exactly one of the two is populated. A real
/// file path is passed through un-read so xberg extracts it (docx, pdf,
/// images via OCR, …) instead of assuming UTF-8 text.
fn read_input(args: &RedactArgs) -> Result<(String, Option<String>)> {
    match (&args.text, &args.file) {
        (Some(t), None) => Ok((t.clone(), None)),
        (None, Some(f)) if f == "-" => {
            let mut buf = Vec::new();
            std::io::stdin().read_to_end(&mut buf).context("read stdin")?;
            Ok((String::from_utf8_lossy(&buf).into_owned(), None))
        }
        (None, Some(path)) => Ok((String::new(), Some(path.clone()))),
        _ => anyhow::bail!("exactly one of --text or --file is required"),
    }
}

/// Runs `basemind redact`: builds `RedactTextParams` from the CLI flags and
/// calls the same `redact_text` tool method the MCP server exposes.
///
/// Refused inside a protected workspace (one carrying a `safe/` mirror): the redaction map
/// this command returns is the token→original table, which belongs to the privacy gateway
/// there. The gateway reaches the engine with the capability it holds
/// (`BASEMIND_GATEWAY_TOKEN`); an agent confined to the mirror does not.
pub async fn run(server: &BasemindServer, args: &RedactArgs, opts: &render::Emit, out: &mut impl Write) -> Result<()> {
    if server.workspace_protected() {
        return Err(crate::cli::exit::CliExit::usage(crate::protected::withheld_message(
            "redact_text",
        )));
    }
    let (text, file_path) = read_input(args)?;
    let params = RedactTextParams {
        text,
        file_path,
        categories: args.categories.clone(),
        strategy: Some(args.strategy.clone()),
        custom_terms: parse_pairs(args.custom_terms.clone())?,
        custom_patterns: parse_pairs(args.custom_patterns.clone())?,
        pseudonyms: parse_pairs(args.pseudonyms.clone())?,
        ner_model_dir: args.ner_model_dir.clone(),
        ner_profile: args.ner_profile.clone(),
        require_ner: false,
    };

    let result = server
        .redact_text_cli(params)
        .await
        .map_err(|e| anyhow::anyhow!("redact_text: {e}"))?;

    let value = render::result_to_value(&result)?;
    if opts.json || args.json {
        serde_json::to_writer(&mut *out, &value)?;
        writeln!(out)?;
    } else {
        render::render_human("redact_text", &value, out)?;
    }
    Ok(())
}
