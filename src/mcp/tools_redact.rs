//! The `redact_text` domain tool — redacts arbitrary text through the same
//! xberg pipeline as document extraction.
//!
//! Flow: obtain content (inline `text`, or `file_path` extracted by xberg —
//! any format incl. images via OCR) → snapshot the original content →
//! `redact_capturing_rehydration_map` (rewrites the document and captures the
//! token map) → return `{redacted_text, rehydration_map, detections}` so the
//! caller can store the map and send redacted text onward.

use std::collections::HashMap;
use std::sync::OnceLock;

use rmcp::ErrorData as McpError;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::CallToolResult;
use rmcp::{schemars, tool};
use serde::{Deserialize, Serialize};

use super::BasemindServer;
use super::helpers::record_call;
use xberg::text::redaction;
use xberg::{ExtractInput, ExtractedDocument, extract};

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
    /// Case dictionary: each entry is `["value", "code_name"]`.
    ///
    /// The value is the one that must not reach the agent — the real name of a party, the
    /// name of the project, an address — and the code name is what stands in for it. Each
    /// value is replaced by its code name in the redacted text, whether the detection engine
    /// found it or not; and a code name the engine did redact on its own is put back. The
    /// rehydration map is left untouched, so the caller keeps a reversible record.
    #[serde(default)]
    pub pseudonyms: Vec<Vec<String>>,
    /// Local directory holding GLiNER2 artifacts (`model.safetensors`,
    /// `tokenizer.json`, `encoder_config/config.json`). When set, NER labels
    /// every entry in the resolved profile for redaction; a missing or
    /// unloadable model degrades to pattern-only redaction instead of failing.
    #[serde(default)]
    pub ner_model_dir: Option<String>,
    /// Named, versioned NER label profile. `full@1` preserves the historical
    /// label set; `legal-fr@1` is the original French legal profile and
    /// `legal-fr@2` adds named private places plus document dates. The resolved
    /// profile and ordered labels are returned so callers can reproduce
    /// coverage and latency measurements.
    #[serde(default)]
    pub ner_profile: Option<String>,
    /// Fail with an error when NER did not run (no `ner_model_dir`, model
    /// missing or unloadable, inference error, build without `ner-candle`)
    /// instead of degrading to pattern-only redaction. For callers that must
    /// not release text whose names, companies or places went undetected.
    #[serde(default)]
    pub require_ner: bool,
}

#[rmcp::tool_router(vis = "pub(super)", router = "tool_router_redact_text")]
impl BasemindServer {
    #[tool(
        title = "Redact text",
        description = "Redact arbitrary text (inline `text`, or a document `file_path` extracted by xberg — PDF/Office/HTML/images via OCR). Returns redacted_text, rehydration_map (token to original), detections (category, start, end, text; start/end are UTF-8 byte offsets into the text as sent for inline text, or into the extracted text for a file_path), the resolved named/versioned `ner_profile` and `ner_ran` (false when NER did not run and only pattern redaction applied; set require_ner to fail instead). When NER ran it windows internally (#71) and reports ner_windows (windows read), ner_window_chars (largest window, in characters) and ner_truncated (true when the text could not be fully read — require_ner fails then too; with ner_truncated false those three fields vouch that every character was read by NER).",
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
/// Raw `file_path` input cap, checked before extraction. MAX_BYTES bounds only
/// the extracted text; without this, a huge file reaches xberg's PDF/image/OCR
/// work before the post-extraction check can reject it.
const MAX_FILE_BYTES: u64 = 64 << 20; // 64 MiB
/// Case-dictionary bounds. Every entry is compiled into a `Regex` and swept over
/// the whole (up to MAX_BYTES) document, so an unbounded entry count or an
/// unbounded value length would make a single call an unbounded CPU sink.
/// Fail the call outright — a value the caller meant to hide must never be
/// partially handled.
const MAX_PSEUDONYM_ENTRIES: usize = 64;
const MAX_PSEUDONYM_VALUE_CHARS: usize = 512;

fn oversized_err(len: usize) -> McpError {
    McpError::internal_error(
        format!("redact_text input too large: {len} bytes (max {MAX_BYTES})"),
        None,
    )
}

/// Where the text to redact comes from.
enum Source {
    /// A file xberg extracts (any format).
    File(Box<ExtractInput>),
    /// Text the caller sent. It is used as is: xberg's plain-text extractor
    /// trims every paragraph, collapses blank-line runs, turns CRLF into LF and
    /// folds decomposed accents, so offsets reported on its output no longer
    /// index what the caller sent (#70).
    Inline(String),
}

/// Body of the `redact_text` tool: reads the inline text or extracts the
/// file, turns NER mentions into boundary-aware custom patterns, then lets
/// xberg redact them together with its own pattern detectors in one pass.
/// When NER cannot run it falls back to pattern-only redaction and reports
/// `ner_ran: false`, or returns an error if `require_ner` is set.
async fn run_redact(args: RedactTextParams) -> Result<CallToolResult, McpError> {
    let ner_profile =
        resolve_ner_profile(args.ner_profile.as_deref()).map_err(|reason| McpError::invalid_params(reason, None))?;
    let source = match args.file_path.as_deref() {
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
                return Err(McpError::internal_error(format!("{path} is not a regular file"), None));
            }
            if meta.len() > MAX_FILE_BYTES {
                return Err(McpError::internal_error(
                    format!("{path} too large: {} bytes (max {MAX_FILE_BYTES})", meta.len()),
                    None,
                ));
            }
            Source::File(Box::new(ExtractInput::from_uri(path.to_string())))
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
            Source::Inline(args.text)
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

    let mut custom_patterns: Vec<RedactionCustomPattern> = args
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
    // Phone formats xberg's built-in pattern misses (international `+CC`
    // grouping, French pairs) — always on, like the pattern engine itself.
    custom_patterns.extend(RedactionConfig::phone_patterns());

    let extraction_config = xberg::core::config::ExtractionConfig {
        redaction: None,
        ..Default::default()
    };

    let mut doc = match source {
        Source::File(input) => {
            let mut extraction = extract(*input, &extraction_config)
                .await
                .map_err(|e| McpError::internal_error(format!("xberg extract failed: {e}"), None))?;
            extraction
                .results
                .pop()
                .ok_or_else(|| McpError::internal_error("xberg returned no extracted document".to_string(), None))?
        }
        Source::Inline(text) => {
            // The struct has private fields, so it is built by default and then filled in.
            let mut doc = ExtractedDocument::default();
            doc.content = text;
            doc.mime_type = "text/plain".into();
            doc
        }
    };
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
    let source_content = doc.content.clone();
    let pdf_offsets = if doc.mime_type.as_ref() == "application/pdf" {
        let trimmed = trim_pdf_line_padding_with_map(&doc.content);
        doc.content = trimmed.text;
        Some(trimmed.offsets)
    } else {
        None
    };
    let original = doc.content.clone();

    // The built-in pattern engine can only express the shape of a French NIR;
    // checksum validation belongs at this boundary. Add exact patterns only
    // for valid candidates so the complete identifier wins over a shorter
    // overlapping phone match without turning every 15-digit run into a NIR.
    custom_patterns.extend(valid_fr_nir_patterns(&original));

    // #37 GLiNER2: label mentions before redaction — the model's 42 PII labels
    // plus AI_ACT_NER_LABELS, with organization/location kept as extra
    // zero-shot labels so the three categories redaction claimed before #37
    // keep working (neither is among the 42).
    // Every failure mode (missing model dir, unloadable model, inference error,
    // build without `ner-candle`) degrades to pattern-only redaction, reported
    // as `ner_ran: false` — or refused outright under `require_ner`.
    // #71: the backend reads only the encoder's first few hundred tokens, so
    // `run_ner_windowed` hands it the text in windows and reports exactly what
    // ran: window count, largest window and whether planning had to stop.
    let mut ner_run = match run_ner_windowed(args.ner_model_dir.as_deref(), &original, &ner_profile).await {
        Ok(run) => run,
        Err(reason) if args.require_ner => {
            return Err(McpError::internal_error(
                format!("NER required but did not run: {reason}"),
                None,
            ));
        }
        Err(_) => NerRun::default(),
    };
    if ner_run.truncated && args.require_ner {
        return Err(McpError::internal_error(
            format!(
                "NER required but the whole text could not be read: detection stops at \
                 byte {} of {}, the token budget would not cover the rest",
                ner_run.cut_off.unwrap_or(0),
                original.len()
            ),
            None,
        ));
    }
    ner_run.entities = merge_span_windows(&original, std::mem::take(&mut ner_run.mentions));
    let ner_confidence: std::collections::HashMap<(u32, u32), f32> = ner_run
        .entities
        .iter()
        .filter_map(|entity| Some(((entity.start, entity.end), entity.confidence?)))
        .collect();
    let legal_fr_v2 = ner_profile.name == "legal-fr@2";
    let mut seen_terms: std::collections::HashSet<(String, String)> = custom_terms
        .iter()
        .map(|term| (term.label.clone(), term.value.clone()))
        .collect();
    for entity in &ner_run.entities {
        let Some(label) = ner_label(&entity.category) else {
            continue;
        };
        if entity.text.chars().count() < MIN_NER_SPAN_CHARS {
            continue;
        }
        if legal_fr_v2
            && should_skip_legal_fr_v2_ner_span(&original, entity.start as usize, entity.end as usize, &label)
        {
            continue;
        }
        if label == "tax_id" && !crate::pii::is_fr_tax_id(&entity.text) {
            // `legal-fr@1` asks the model for tax IDs, but a low-confidence
            // model span is not a tax identifier by itself. Keep the format
            // gate here so invoice references do not become tax-id tokens.
            continue;
        }
        if seen_terms.insert((label.clone(), entity.text.clone())) {
            // Boundary-aware regex, not a literal term: xberg matches terms as
            // substrings, so a span like "US" would corrupt every "because".
            custom_patterns.push(RedactionCustomPattern {
                label,
                pattern: ner_span_pattern(&entity.text),
                case_sensitive: false,
            });
        }
    }

    // AI Act citations ride the same seam: exact literals, so a regex rather
    // than a zero-shot label. Custom patterns are retained even when the caller
    // asks for a narrow `categories` list (xberg keeps every `Custom` match).
    custom_patterns.push(RedactionCustomPattern {
        label: "ai_act_citation".to_string(),
        pattern: AI_ACT_CITATION_REGEX.to_string(),
        case_sensitive: false,
    });

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

    // xberg's report includes findings from metadata, chunks, and other secondary fields whose
    // offsets are relative to those fields, not `content`. Redact a content-only document so the
    // returned detections and rehydration map use one coordinate system.
    let mut content_doc = ExtractedDocument::default();
    content_doc.content = original.clone();
    content_doc.mime_type = doc.mime_type.clone();
    let map = redaction::redact_capturing_rehydration_map(&mut content_doc, &redaction_config)
        .await
        .map_err(|e| McpError::internal_error(format!("xberg redaction failed: {e}"), None))?;

    let mut findings = content_doc
        .redaction_report
        .map(|report| report.findings)
        .unwrap_or_default();
    let mut rehydration_map: std::collections::BTreeMap<String, String> = map.into_iter().collect();
    let mut redacted_text = content_doc.content;

    // xberg's built-in phone/BIC regexes intentionally favour recall. The
    // legal gateway needs the same conservative boundary for obvious date and
    // prose-shaped matches; rebuild the content from the original after
    // dropping only those findings, preserving every accepted replacement and
    // its caller-owned token map.
    let before_filter = findings.len();
    let legal_fr_filters = matches!(ner_profile.name.as_str(), "legal-fr@1" | "legal-fr@2");
    findings.retain(|finding| !is_false_positive_pattern(&original, finding, legal_fr_filters));
    if findings.len() != before_filter {
        redacted_text = rebuild_redacted_text(&original, &findings);
        rehydration_map.retain(|token, _| findings.iter().any(|finding| finding.replacement_token == *token));
    }

    // A French company identifier (SIREN, SIRET) is a run of digits the engine files as a
    // phone number. The legal caller acts on the label, so it has to be `tax_id`.
    crate::pii::relabel_in_text(&mut findings, &original);
    // …and the token in the text carries that label too: a company identifier is not
    // `[PHONE_3]`.
    crate::pii::rename_tokens(&mut redacted_text, &mut rehydration_map, &findings);

    // The case dictionary: the value becomes its code name, whether the engine found it or
    // not. Fail closed: an entry the caller meant to hide must never be silently dropped —
    // a malformed pair aborts the call rather than leaving the real value standing in the
    // text the agent receives.
    let mut pseudonyms_applied: Option<serde_json::Value> = None;
    // The bounds: beyond them the call is refused, not truncated — a value the caller
    // meant to hide must never be partially handled.
    if args.pseudonyms.len() > MAX_PSEUDONYM_ENTRIES {
        return Err(McpError::invalid_params(
            format!(
                "too many pseudonyms: {} entries (max {MAX_PSEUDONYM_ENTRIES})",
                args.pseudonyms.len()
            ),
            None,
        ));
    }
    let dictionary: Vec<crate::pii::Pseudonym> = args
        .pseudonyms
        .iter()
        .map(|entry| {
            if entry.len() != 2 {
                return Err(McpError::invalid_params(
                    format!(
                        "each pseudonym entry is a [value, code_name] pair; got {} element(s)",
                        entry.len()
                    ),
                    None,
                ));
            }
            for element in [&entry[0], &entry[1]] {
                if element.chars().count() > MAX_PSEUDONYM_VALUE_CHARS {
                    return Err(McpError::invalid_params(
                        format!(
                            "pseudonym entry too long: {} characters (max {MAX_PSEUDONYM_VALUE_CHARS})",
                            element.chars().count()
                        ),
                        None,
                    ));
                }
            }
            crate::pii::Pseudonym::new(entry[0].clone(), entry[1].clone()).ok_or_else(|| {
                McpError::invalid_params(
                    "a pseudonym's value and code name must both be non-empty".to_string(),
                    None,
                )
            })
        })
        .collect::<Result<Vec<_>, McpError>>()?;
    if !dictionary.is_empty() {
        // How many occurrences the engine produced per token: the bound the dictionary
        // sweep replaces under, so token notation the document itself quotes ("then
        // read [PERSON_1] here") is not silently rewritten into a code name.
        let mut token_counts: HashMap<String, usize> = HashMap::new();
        for finding in &findings {
            *token_counts.entry(finding.replacement_token.clone()).or_default() += 1;
        }
        let (substituted, applied) =
            crate::pii::apply_pseudonyms_with_counts(&redacted_text, &rehydration_map, &dictionary, &token_counts);
        redacted_text = substituted;
        // recorded in the payload below, beside the NER coverage fields
        pseudonyms_applied = Some(serde_json::json!({
            "tokens": applied.tokens,
            "literals": applied.literals,
        }));
    }

    let detections: Vec<serde_json::Value> = findings
        .iter()
        .map(|finding| {
            let start = finding.start as usize;
            let end = finding.end as usize;
            let source_start = pdf_offsets
                .as_ref()
                .and_then(|offsets| offsets.get(start).copied())
                .unwrap_or(start);
            let source_end = pdf_offsets
                .as_ref()
                .and_then(|offsets| offsets.get(end).copied())
                .unwrap_or(end);
            let mut detection = serde_json::json!({
                "category": detection_category(&finding.category),
                "start": source_start,
                "end": source_end,
                "text": source_content.get(source_start..source_end).unwrap_or("")
            });
            // Honest confidence: only NER-derived spans carry one, keyed by the
            // exact span the model reported; anything else omits the field.
            if let Some(confidence) = ner_confidence.get(&(finding.start, finding.end)) {
                detection["confidence"] = serde_json::json!(confidence);
            }
            detection
        })
        .collect();

    let mut payload = serde_json::json!({
        "redacted_text": redacted_text,
        "rehydration_map": rehydration_map,
        "detections": detections,
        "ner_ran": ner_run.ran,
        "ner_profile": {
            "name": ner_profile.name,
            "labels": ner_profile.labels,
        },
    });
    // #71 coverage contract: a result that carries `ner_windows` and
    // `ner_window_chars` vouches that the model read (only) those windows — with
    // `ner_truncated: false` that is the whole text. A nonexistent model keeps
    // the fields off: only `ner_ran: false` is honest about what happened.
    if ner_run.ran {
        payload["ner_windows"] = serde_json::json!(ner_run.windows);
        payload["ner_window_chars"] = serde_json::json!(ner_run.window_chars);
        payload["ner_truncated"] = serde_json::json!(ner_run.truncated);
    }
    // The dictionary substitutions, when any: engine redaction (a token,
    // rehydrated through `rehydration_map`) is told apart from dictionary
    // substitution (the lawyer's code name), so a dispute over what the agent
    // was shown has an auditable record.
    if let Some(applied) = pseudonyms_applied {
        payload["pseudonyms_applied"] = applied;
    }
    Ok(CallToolResult::structured(payload))
}

/// Plain-string detection category. Native PII categories serialise to
/// `"person"` already; custom labels (NER terms, `--custom-term`) serialise to
/// `{"custom": …}`, which naive `as_str` handling would report as "unknown".
fn detection_category(category: &xberg::types::redaction::PiiCategory) -> String {
    match serde_json::to_value(category) {
        Ok(serde_json::Value::String(label)) => label,
        Ok(serde_json::Value::Object(map)) => map
            .get("custom")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| "unknown".to_string()),
        _ => "unknown".to_string(),
    }
}

/// PDF text extraction pads short lines to a page-width column. Those spaces
/// contain no document information and otherwise multiply the NER windows.
struct TrimmedPdfText {
    text: String,
    /// For every UTF-8 byte boundary in `text`, the corresponding boundary in
    /// the original extracted PDF text.
    offsets: Vec<usize>,
}

/// Trim PDF column padding while retaining a byte-boundary map to the extracted source.
fn trim_pdf_line_padding_with_map(text: &str) -> TrimmedPdfText {
    let mut normalized = String::with_capacity(text.len());
    let mut offsets = vec![0];
    let mut source_start = 0;
    for line in text.split_inclusive('\n') {
        let has_newline = line.ends_with('\n');
        let line_without_newline = line.strip_suffix('\n').unwrap_or(line);
        let has_carriage_return = line_without_newline.ends_with('\r');
        let body = line_without_newline.strip_suffix('\r').unwrap_or(line_without_newline);
        let body_end = source_start + body.len();
        let trimmed_end = text[source_start..body_end]
            .trim_end_matches([' ', '\t'])
            .len()
            .saturating_add(source_start);

        normalized.push_str(&text[source_start..trimmed_end]);
        offsets.extend((source_start + 1)..=trimmed_end);
        if has_carriage_return {
            normalized.push('\r');
            offsets.push(body_end + 1);
        }
        if has_newline {
            normalized.push('\n');
            offsets.push(source_start + line.len());
        }
        source_start += line.len();
    }
    TrimmedPdfText {
        text: normalized,
        offsets,
    }
}

fn valid_fr_nir_patterns(text: &str) -> Vec<RedactionCustomPattern> {
    let regex = regex::Regex::new(
        r"\b[12]\p{White_Space}?\d{2}\p{White_Space}?(?:0[1-9]|1[0-2])\p{White_Space}?(?:\d{2}|2[AB])\p{White_Space}?\d{3}\p{White_Space}?\d{3}\p{White_Space}?\d{2}\b",
    )
    .expect("the static French NIR regex must compile");
    regex
        .find_iter(text)
        .filter(|candidate| crate::pii::validate_fr_nir(candidate.as_str()))
        .map(|candidate| RedactionCustomPattern {
            label: "national_id_number".to_string(),
            pattern: format!(r"\b{}\b", regex::escape(candidate.as_str())),
            case_sensitive: true,
        })
        .collect()
}

/// Decide whether a high-recall engine finding is safe to discard in this legal context.
fn is_false_positive_pattern(
    original: &str,
    finding: &xberg::types::redaction::RedactionFinding,
    legal_fr_filters: bool,
) -> bool {
    let start = finding.start as usize;
    let end = finding.end as usize;
    let Some(value) = original.get(start..end) else {
        return false;
    };
    match &finding.category {
        xberg::types::redaction::PiiCategory::Phone => {
            // French phones have ten national digits; an eight-digit run is a
            // reference/date pair in this gateway's legal corpus, not a phone.
            legal_fr_filters && is_legal_reference_shape(value) && has_reference_or_date_cue(original, start, end)
        }
        xberg::types::redaction::PiiCategory::SwiftBic => {
            // All-letter BIC-shaped words need a nearby banking cue. This
            // avoids masking ordinary uppercase prose such as ATTESTATION.
            value.chars().all(|c| c.is_ascii_alphabetic()) && !has_bic_context(original, start, end)
        }
        _ => false,
    }
}

fn is_legal_reference_shape(value: &str) -> bool {
    let compact_date = value.len() == 8 && value.chars().all(|c| c.is_ascii_digit());
    let year_reference = value.split_once('-').is_some_and(|(year, suffix)| {
        year.len() == 4
            && matches!(year.as_bytes().first(), Some(b'1' | b'2'))
            && year.chars().all(|c| c.is_ascii_digit())
            && (3..=4).contains(&suffix.len())
            && suffix.chars().all(|c| c.is_ascii_digit())
    });
    compact_date || year_reference
}

/// Decide whether a `legal-fr@2` model span is a known false positive. The
/// profile is deliberately conservative: broad geographic scope remains
/// readable, while named private places are sent through the model as the
/// narrower `named_place` label.
fn should_skip_legal_fr_v2_ner_span(text: &str, start: usize, end: usize, label: &str) -> bool {
    let Some(value) = text.get(start..end) else {
        return false;
    };
    let normalized = normalize_ner_value(value);
    match label {
        "named_place" => is_broad_geography(&normalized),
        "organization" => GENERIC_LEGAL_ORGANIZATIONS.contains(&normalized.as_str()),
        "iban" => !crate::pii::validate_iban(value),
        "date_of_birth" => !has_birth_context(text, start, end),
        _ => false,
    }
}

/// Test-facing shorthand for a span whose value is the complete input.
#[cfg(test)]
fn should_skip_legal_fr_v2_ner_value(value: &str, label: &str) -> bool {
    should_skip_legal_fr_v2_ner_span(value, 0, value.len(), label)
}

const GENERIC_LEGAL_ORGANIZATIONS: &[&str] = &["sécurité sociale", "ma société", "siège social", "employeur"];

/// Geography that describes a broad legal scope, not a private named place.
/// Keep this list explicit: treating every one-word model span as a country or
/// region would let a private site name pass through the gateway.
fn is_broad_geography(value: &str) -> bool {
    const BROAD_GEOGRAPHY: &[&str] = &[
        "europe",
        "union européenne",
        "france métropolitaine",
        "grand sud-ouest",
        "sud-ouest",
        "france",
        "allemagne",
        "angleterre",
        "belgique",
        "espagne",
        "italie",
        "pays-bas",
        "portugal",
        "suisse",
        "royaume-uni",
        "afrique",
        "asie",
        "amérique",
        "océanie",
    ];
    BROAD_GEOGRAPHY.contains(&value)
}

fn normalize_ner_value(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

fn has_birth_context(text: &str, start: usize, end: usize) -> bool {
    let (context_start, context_end) = aligned_context_bounds(text, start, end);
    let Some(context) = text.get(context_start..context_end).map(str::to_lowercase) else {
        return false;
    };
    [
        "date de naissance",
        "lieu de naissance",
        "né le",
        "née le",
        "né(e)",
        "naissance",
        "birth",
        "born",
    ]
    .iter()
    .any(|cue| context.contains(cue))
}

/// Expand a byte window to valid UTF-8 boundaries before it is sliced.
fn aligned_context_bounds(text: &str, start: usize, end: usize) -> (usize, usize) {
    let mut context_start = start.saturating_sub(80).min(text.len());
    while context_start > 0 && !text.is_char_boundary(context_start) {
        context_start -= 1;
    }
    let mut context_end = end.saturating_add(80).min(text.len());
    while context_end < text.len() && !text.is_char_boundary(context_end) {
        context_end += 1;
    }
    (context_start, context_end)
}

/// Detect the nearby reference/date cues that distinguish eight-digit references from phones.
fn has_reference_or_date_cue(text: &str, start: usize, end: usize) -> bool {
    let (context_start, context_end) = aligned_context_bounds(text, start, end);
    let Some(context) = text.get(context_start..context_end).map(str::to_lowercase) else {
        return false;
    };
    const REFERENCE_CUES: &[&str] = &[
        "ref",
        "reference",
        "référence",
        "dossier",
        "case",
        "matter",
        "n°",
        "no.",
        "annonce",
        "bodacc",
        "facture",
        "devis",
        "publié",
        "période",
    ];
    if REFERENCE_CUES.iter().any(|cue| context.contains(cue)) {
        return true;
    }
    static DATE_CUE: OnceLock<regex::Regex> = OnceLock::new();
    DATE_CUE
        .get_or_init(|| regex::Regex::new(r"\b\d{1,2}[./-]\d{1,2}[./-]\d{2,4}\b").expect("date cue regex"))
        .is_match(&context)
}

fn has_bic_context(text: &str, start: usize, end: usize) -> bool {
    let (context_start, context_end) = aligned_context_bounds(text, start, end);
    text.get(context_start..context_end)
        .map(str::to_ascii_lowercase)
        .is_some_and(|context| ["bic", "swift", "iban"].iter().any(|cue| context.contains(cue)))
}

fn rebuild_redacted_text(original: &str, findings: &[xberg::types::redaction::RedactionFinding]) -> String {
    let mut ordered = findings.to_vec();
    ordered.sort_by_key(|finding| (finding.start, finding.end));
    let mut rebuilt = String::with_capacity(original.len());
    let mut cursor = 0usize;
    for finding in ordered {
        let start = finding.start as usize;
        let end = finding.end as usize;
        if start < cursor
            || end > original.len()
            || !original.is_char_boundary(start)
            || !original.is_char_boundary(end)
        {
            continue;
        }
        rebuilt.push_str(&original[cursor..start]);
        rebuilt.push_str(&finding.replacement_token);
        cursor = end;
    }
    rebuilt.push_str(&original[cursor..]);
    rebuilt
}

/// Entity category → the redaction label safe/ redaction claims.
///
/// GLiNER2 is asked for `GLI_NER2_PII_LABELS` and `AI_ACT_NER_LABELS` as
/// `Custom(label)`, so every span it finds arrives here under the label it
/// was requested with. Only `person`, `organization`, `location` and `email`
/// round-trip through `EntityCategory::from` to a named variant; `None` covers
/// the remaining named variants (date, url, …) the model was never asked for.
fn ner_label(category: &xberg::types::entity::EntityCategory) -> Option<String> {
    use xberg::types::entity::EntityCategory;
    match category {
        EntityCategory::Person => Some("person".to_string()),
        EntityCategory::Organization => Some("organization".to_string()),
        EntityCategory::Location => Some("location".to_string()),
        EntityCategory::Email => Some("email".to_string()),
        EntityCategory::Custom(label) if !label.is_empty() => Some(label.clone()),
        _ => None,
    }
}

/// The 42 labels `fastino/gliner2-privacy-filter-PII-multi` was trained on
/// (7 languages: en, fr, es, de, it, pt, nl). GLiNER2 conditions on the label
/// list given at inference time, so this list — not the model — decides what
/// NER can find.
const GLI_NER2_PII_LABELS: &[&str] = &[
    // person / names
    "person",
    "full_name",
    "first_name",
    "middle_name",
    "last_name",
    "date_of_birth",
    // contact / address
    "email",
    "phone_number",
    "address",
    "street_address",
    "city",
    "state_or_region",
    "postal_code",
    "country",
    // government / tax IDs
    "government_id",
    "national_id_number",
    "passport_number",
    "drivers_license_number",
    "license_number",
    "tax_id",
    "tax_number",
    // banking / payment
    "bank_account",
    "account_number",
    "routing_number",
    "iban",
    "payment_card",
    "card_number",
    "card_expiry",
    "card_cvv",
    // digital identity
    "username",
    "ip_address",
    "account_id",
    "sensitive_account_id",
    // secrets / credentials
    "password",
    "secret",
    "api_key",
    "access_token",
    "recovery_code",
    // sensitive dates
    "sensitive_date",
    "document_date",
    "expiration_date",
    "transaction_date",
];

/// EU AI Act terms the PII model never saw in training. Zero-shot, so they ride
/// the same candle backend as the 42 PII labels — no separate model.
///
/// Spans, not concepts: a zero-shot label matches the text it names
/// ("provider of the high-risk AI system"), which is why these read as
/// `ai_act_<thing>` rather than a bare role word that would fire on every
/// "cloud provider" in sight.
const AI_ACT_NER_LABELS: &[&str] = &[
    // actor roles
    "ai_act_provider",
    "ai_act_deployer",
    "ai_act_importer",
    "ai_act_distributor",
    "ai_act_notified_body",
    "ai_act_authorised_representative",
    // risk classes
    "high_risk_ai_system",
    "gpai_model",
    "ai_act_systemic_risk",
    // obligations & penalties
    "ai_act_conformity_assessment",
    "ai_act_technical_documentation",
    "ai_act_market_surveillance",
    "ai_act_penalty",
];

/// AI Act citation literals: the regulation's own number, `2024/1689`.
/// Regex, not NER — an exact string, and a model adds nothing to an exact
/// match. Scoped to that number instead of any `YYYY/NNNN` regulation or the
/// bare phrase "AI Act", so ordinary prose ("the AI Act requires...") and
/// other regulations (`2022/2065` = the Digital Services Act) pass through
/// untouched. The AI Act's roles, obligations and penalties are zero-shot
/// labels, not this pattern — see `AI_ACT_NER_LABELS`.
const AI_ACT_CITATION_REGEX: &str = r"\b2024/1689\b";

/// NER spans shorter than this are dropped: even whole-word, case-insensitive
/// hits on 1-2 char spans ("us", "go") are noise that would shred the mirror.
const MIN_NER_SPAN_CHARS: usize = 3;

/// The labels sent to GLiNER2 are part of the coverage contract: changing
/// them changes both what the model can detect and the window token budget.
/// Keep profiles named and versioned so a gateway can pin one and reproduce a
/// result instead of relying on an implicit process default.
#[derive(Debug, Clone)]
struct NerProfile {
    name: String,
    labels: Vec<String>,
}

const LEGAL_FR_LABELS: &[&str] = &[
    "full_name",
    "first_name",
    "last_name",
    "person",
    "organization",
    "address",
    "street_address",
    "city",
    "postal_code",
    "email",
    "phone_number",
    "iban",
    "bank_account",
    "tax_id",
    "national_id_number",
    "date_of_birth",
];

fn resolve_ner_profile(requested: Option<&str>) -> Result<NerProfile, String> {
    let name = requested.unwrap_or("full@1");
    let labels = match name {
        "full@1" => {
            let mut labels = vec!["organization".to_string(), "location".to_string()];
            labels.extend(GLI_NER2_PII_LABELS.iter().map(|label| (*label).to_string()));
            labels.extend(AI_ACT_NER_LABELS.iter().map(|label| (*label).to_string()));
            labels
        }
        "legal-fr@1" => LEGAL_FR_LABELS.iter().map(|label| (*label).to_string()).collect(),
        "legal-fr@2" => {
            let mut labels: Vec<String> = LEGAL_FR_LABELS.iter().map(|label| (*label).to_string()).collect();
            labels.extend(
                ["named_place", "document_date"]
                    .iter()
                    .map(|label| (*label).to_string()),
            );
            labels
        }
        other => {
            return Err(format!(
                "unknown NER profile {other}; expected full@1, legal-fr@1 or legal-fr@2"
            ));
        }
    };
    Ok(NerProfile {
        name: name.to_string(),
        labels,
    })
}

/// Boundary-aware escaped pattern for a NER span, so matches stay whole-word.
/// `\b` is emitted only at edges that start/end a word char — a span ending
/// in punctuation ("Acme Corp.") needs no trailing `\b`, and one there would
/// never match (no boundary exists between `.` and a space).
fn ner_span_pattern(text: &str) -> String {
    let escaped = regex::escape(text);
    let start = if text.starts_with(is_regex_word) { r"\b" } else { "" };
    let end = if text.ends_with(is_regex_word) { r"\b" } else { "" };
    format!("{start}{escaped}{end}")
}

/// Mirrors the regex crate's `\w` (word char) definition for edge decisions.
fn is_regex_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// What one `redact_text` call's NER produced.
///
/// When the model did not run (`ran: false`), every field is the default: NER
/// `window_chars` (largest window, in characters), `truncated` — true only
/// when reading had to stop before covering the text: planning that could not
/// pay for the rest, or the wall-clock budget — in which case the caller
/// reports `ner_truncated: true` and `require_ner` fails the call.
///
/// Until the merge runs, mentions are tagged with the window that found each
/// (`None` once a merged span crosses windows): the merge needs the
/// provenance, the caller does not.
#[derive(Default)]
struct NerRun {
    mentions: Vec<(xberg::types::entity::Entity, Option<usize>)>,
    entities: Vec<xberg::types::entity::Entity>,
    ran: bool,
    windows: usize,
    window_chars: usize,
    truncated: bool,
    cut_off: Option<usize>,
}

/// Merge mentions across windows into the redaction-claim set: same label to
/// its union, rival labels to the more confident winner (#71). Identity when
/// nothing ran (no `ner-candle` build, no model): the empty list in, the empty
/// list out.
#[cfg(feature = "ner-candle")]
fn merge_span_windows(
    original: &str,
    mentions: Vec<(xberg::types::entity::Entity, Option<usize>)>,
) -> Vec<xberg::types::entity::Entity> {
    super::ner_windowing::merge_window_entities(original, mentions)
}

#[cfg(not(feature = "ner-candle"))]
fn merge_span_windows(
    _original: &str,
    entities: Vec<(xberg::types::entity::Entity, Option<usize>)>,
) -> Vec<xberg::types::entity::Entity> {
    entities.into_iter().map(|(entity, _)| entity).collect()
}

/// The same labels as the categories the candle backend receives. Named
/// variants keep their round-trip labels (`person`, `organization`, …);
/// everything else is a zero-shot `Custom` label.
#[cfg(feature = "ner-candle")]
fn ner_categories(labels: &[String]) -> Vec<xberg::types::entity::EntityCategory> {
    use xberg::types::entity::EntityCategory;
    labels
        .iter()
        .map(|label| match label.as_str() {
            "organization" => EntityCategory::Organization,
            "location" => EntityCategory::Location,
            other => EntityCategory::Custom(other.to_string()),
        })
        .collect()
}

/// Detect PII and AI Act mentions with GLiNER2, windowed (#71): the backend's
/// candle path reads only the encoder's first few hundred tokens (schema
/// prompt included) and silently drops the rest, so a single call covered only
/// the head of any longer text. The text is windowed here — sized from the
/// encoder's own token budget, cut at word boundaries, one window per call —
/// and every span is shifted back onto the caller's offsets (UTF-8 bytes, the
/// #70 contract).
///
/// `Ok` means the model ran; the run's `truncated` field is honest about
/// partial coverage when even windowing could not fit the token budget. `Err`
/// carries why nothing ran: no dir, model unavailable, budget unknown, or an
/// inference failure (the mention set is then all-or-nothing; a mid-window
/// failure cannot vouch for the windows before it, so none is kept).
#[cfg(feature = "ner-candle")]
async fn run_ner_windowed(model_dir: Option<&str>, text: &str, profile: &NerProfile) -> Result<NerRun, String> {
    use crate::mcp::ner_windowing;
    use std::time::{Duration, Instant};
    use xberg::text::ner::NerBackend;
    use xberg::text::ner::candle::CandleBackend;
    /// Wall-clock budget for the whole NER pass inside one call. Windows run
    /// sequentially, so an extremely large input could otherwise hold an MCP
    /// call for hours (the app measured roughly 4 s per small window on a
    /// CPU-only i5-7360U; 1 MiB would be thousands). Past the deadline
    /// the run reports truncation exactly like a budget-stopped plan:
    /// `require_ner` fails, the pattern pass stands, nothing is cut silently.
    const NER_TIME_BUDGET: Duration = Duration::from_secs(10 * 60);
    let Some(dir) = model_dir else {
        return Err("no ner_model_dir given".to_string());
    };
    let labels = profile.labels.clone();
    let categories = ner_categories(&labels);
    let backend = match CandleBackend::get_or_init(std::path::Path::new(dir), None) {
        Ok(backend) => backend,
        Err(error) => {
            tracing::warn!(%error, dir, "GLiNER2 model unavailable; redacting pattern-only");
            return Err(format!("model unavailable: {error}"));
        }
    };
    let budget = match ner_windowing::load_budget(dir, &labels) {
        Ok(budget) => budget,
        Err(reason) => {
            tracing::warn!(%reason, dir, "NER window budget unknown; redacting pattern-only");
            return Err(reason);
        }
    };
    let plan = match ner_windowing::plan_windows(text, &budget) {
        Ok(plan) => plan,
        Err(reason) => {
            tracing::warn!(%reason, dir, "NER could not window the text; redacting pattern-only");
            return Err(reason);
        }
    };
    tracing::debug!(
        dir,
        text_bytes = text.len(),
        prompt_tokens = budget.prompt_tokens,
        tokens_for_text = budget.tokens_for_text,
        windows = plan.windows.len(),
        truncated = plan.cut_off.is_some(),
        "NER windows planned"
    );
    let mut run = NerRun {
        ran: true,
        windows: plan.windows.len(),
        truncated: plan.cut_off.is_some(),
        cut_off: plan.cut_off,
        ..Default::default()
    };
    let started = Instant::now();
    let mut read_windows = 0usize;
    for (index, window) in plan.windows.iter().enumerate() {
        if started.elapsed() >= NER_TIME_BUDGET {
            tracing::warn!(
                after_windows = index,
                elapsed_s = started.elapsed().as_secs(),
                remaining = plan.windows.len() - index,
                "NER window deadline reached; windows remain unrun, reporting truncation",
            );
            run.cut_off = Some(run.cut_off.unwrap_or(window.start));
            break;
        }
        let Some(slice) = text.get(window.start..window.end) else {
            return Err(format!(
                "window [{}, {}) does not fall on character boundaries",
                window.start, window.end
            ));
        };
        match backend.detect(slice, &categories).await {
            Ok(entities) => {
                let shifted = entities.into_iter().map(|mut entity| {
                    entity.start += window.start as u32;
                    entity.end += window.start as u32;
                    (entity, Some(index))
                });
                run.mentions.extend(shifted);
            }
            Err(error) => {
                tracing::warn!(%error, "GLiNER2 NER failed; redacting pattern-only");
                return Err(format!("inference failed: {error}"));
            }
        }
        run.window_chars = run.window_chars.max(slice.chars().count());
        read_windows += 1;
    }
    // `ner_windows` says how many windows were actually read, so a
    // deadline-stopped run claims fewer than were planned.
    run.windows = read_windows;
    run.truncated = run.cut_off.is_some();
    Ok(run)
}

/// Stand-in for builds without `ner-candle`: NER never runs, so this always
/// returns `Err` and the caller degrades or refuses per `require_ner`.
#[cfg(not(feature = "ner-candle"))]
async fn run_ner_windowed(model_dir: Option<&str>, _text: &str, _profile: &NerProfile) -> Result<NerRun, String> {
    if let Some(dir) = model_dir {
        tracing::warn!(dir, "built without the ner-candle feature; redacting pattern-only");
    }
    Err("built without the ner-candle feature".to_string())
}

#[cfg(test)]
#[path = "redact_file_tests.rs"]
mod redact_file_tests;
