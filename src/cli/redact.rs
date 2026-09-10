//! `basemind redact` — PII detection and redaction for arbitrary text.
//!
//! Wraps `xberg::text::redaction::redact()` for direct text processing without
//! requiring a git repository scan.

use std::collections::BTreeMap;
use std::io::Write;

use std::io::Read as _;

use anyhow::{Context, Result};
use clap::Args;
use xberg::text::redaction;

use crate::config::{RedactionConfig, RedactionCustomPattern, RedactionCustomTerm, RedactionStrategy};

#[derive(Args, Debug)]
pub struct RedactArgs {
    /// Text to redact. Mutually exclusive with --file.
    #[arg(long, conflicts_with = "file")]
    pub text: Option<String>,

    /// File to read and redact. Mutually exclusive with --text. Use `-` for stdin.
    #[arg(long, conflicts_with = "text", value_name = "FILE")]
    pub file: Option<String>,

    /// Redaction strategy: token-replace (default) | mask | hash | drop.
    /// token-replace → `[TYPE_1]` (reversible via rehydration map)
    /// mask → `[REDACTED]` (irreversible)
    /// hash → `[HASH:...]` (irreversible)
    /// drop → remove the span entirely (irreversible)
    #[arg(long, default_value = "token-replace", value_name = "STRATEGY")]
    pub strategy: String,

    /// Comma-separated list of PII categories to redact. Example:
    /// --categories email,phone,iban
    /// Empty = all supported categories.
    #[arg(long, value_delimiter = ',', value_name = "CATEGORIES")]
    pub categories: Vec<String>,

    /// Custom literal terms to redact. Format: `--custom-term label,value`.
    /// Can be specified multiple times.
    #[arg(long = "custom-term", value_name = "LABEL,VALUE")]
    pub custom_terms: Vec<String>,

    /// Custom regex patterns to redact. Format: `--custom-pattern label,regex`.
    /// Can be specified multiple times.
    #[arg(long = "custom-pattern", value_name = "LABEL,REGEX")]
    pub custom_patterns: Vec<String>,

    /// Output machine-readable JSON regardless of TTY. Default when stdout is not a TTY.
    #[arg(long)]
    pub json: bool,
}

fn parse_custom_terms(raw: Vec<String>) -> Result<Vec<RedactionCustomTerm>> {
    raw.into_iter()
        .map(|s| {
            let parts: Vec<&str> = s.splitn(2, ',').collect();
            anyhow::ensure!(parts.len() == 2, "--custom-term must be label,value (got: {s})");
            Ok(RedactionCustomTerm {
                label: parts[0].to_string(),
                value: parts[1].to_string(),
                case_sensitive: false,
            })
        })
        .collect()
}

fn parse_custom_patterns(raw: Vec<String>) -> Result<Vec<RedactionCustomPattern>> {
    raw.into_iter()
        .map(|s| {
            let parts: Vec<&str> = s.splitn(2, ',').collect();
            anyhow::ensure!(parts.len() == 2, "--custom-pattern must be label,regex (got: {s})");
            Ok(RedactionCustomPattern {
                label: parts[0].to_string(),
                pattern: parts[1].to_string(),
                case_sensitive: false,
            })
        })
        .collect()
}

fn build_config(
    categories: Vec<String>,
    strategy: String,
    custom_terms: Vec<String>,
    custom_patterns: Vec<String>,
) -> Result<RedactionConfig> {
    let strategy = match strategy.as_str() {
        "token-replace" => RedactionStrategy::TokenReplace,
        "mask" => RedactionStrategy::Mask,
        "hash" => RedactionStrategy::Hash,
        "drop" => RedactionStrategy::Drop,
        other => anyhow::bail!("unknown strategy: {other} (expected token-replace|mask|hash|drop)"),
    };
    let mut config = RedactionConfig::default();
    config.enabled = true;
    config.strategy = strategy;
    config.categories = categories;
    config.custom_terms = parse_custom_terms(custom_terms)?;
    config.custom_patterns = parse_custom_patterns(custom_patterns)?;
    Ok(config)
}

/// Read text from --text, --file, or stdin.
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

#[derive(serde::Serialize)]
struct Output {
    redacted_text: String,
    rehydration_map: BTreeMap<String, String>,
    detections: Vec<Detection>,
}

#[derive(serde::Serialize)]
struct Detection {
    category: String,
    start: usize,
    end: usize,
    text: String,
}

pub fn run(args: &RedactArgs, out: &mut impl Write, is_tty: bool) -> Result<()> {
    let text = read_input(args)?;
    let config = build_config(
        args.categories.clone(),
        args.strategy.clone(),
        args.custom_terms.clone(),
        args.custom_patterns.clone(),
    )?;

    let json_mode = args.json || !is_tty;

    let xberg_config = match config.to_xberg() {
        Some(cfg) => cfg,
        None => {
            if json_mode {
                serde_json::to_writer(
                    out,
                    &Output {
                        redacted_text: text,
                        rehydration_map: BTreeMap::new(),
                        detections: vec![],
                    },
                )?;
            } else {
                writeln!(out, "{}", text)?;
            }
            return Ok(());
        }
    };

    // Extract the raw text without redaction, snapshot the original content,
    // then redact while capturing the rehydration map. The engine drops the
    // original bytes, so detections slice the snapshot by finding offsets.
    let base_config = xberg::core::config::ExtractionConfig::default();
    let input = xberg::ExtractInput::from_bytes(text.into_bytes(), "text/plain", Some("input.txt".to_string()));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build extraction runtime")?;
    let mut extraction = runtime
        .block_on(xberg::extract(input, &base_config))
        .context("xberg extract failed")?;
    let mut doc = extraction
        .results
        .pop()
        .context("xberg returned no extracted document")?;
    let original = doc.content.clone();
    let map = runtime
        .block_on(redaction::redact_capturing_rehydration_map(&mut doc, &xberg_config))
        .context("xberg redaction failed")?;

    let findings = doc.redaction_report.map(|report| report.findings).unwrap_or_default();
    let detections: Vec<Detection> = findings
        .iter()
        .map(|finding| {
            let category = serde_json::to_value(&finding.category)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_else(|| "unknown".to_string());
            let start = finding.start as usize;
            let end = finding.end as usize;
            Detection {
                category,
                start,
                end,
                text: original.get(start..end).unwrap_or("").to_string(),
            }
        })
        .collect();

    let rehydration_map: BTreeMap<String, String> = map.into_iter().collect();

    if json_mode {
        let output = Output {
            redacted_text: doc.content,
            rehydration_map,
            detections,
        };
        serde_json::to_writer(out, &output)?;
    } else {
        writeln!(out, "{}", doc.content)?;
    }

    Ok(())
}
