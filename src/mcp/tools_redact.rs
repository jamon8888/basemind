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
    /// every entry in `GLI_NER2_PII_LABELS` plus `AI_ACT_NER_LABELS` for
    /// redaction; a missing or unloadable model degrades to pattern-only
    /// redaction instead of failing.
    #[serde(default)]
    pub ner_model_dir: Option<String>,
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
        description = "Redact arbitrary text (inline `text`, or a document `file_path` extracted by xberg — PDF/Office/HTML/images via OCR). Returns redacted_text, rehydration_map (token to original), detections (category, start, end, text; start/end are UTF-8 byte offsets into the text as sent for inline text, or into the extracted text for a file_path), and ner_ran (false when NER did not run and only pattern redaction applied; set require_ner to fail instead). When NER ran it windows internally (#71) and reports ner_windows (windows read), ner_window_chars (largest window, in characters) and ner_truncated (true when the text could not be fully read — require_ner fails then too; with ner_truncated false those three fields vouch that every character was read by NER).",
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
    let original = doc.content.clone();

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
    let mut ner_run = match run_ner_windowed(args.ner_model_dir.as_deref(), &original).await {
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

    let map = redaction::redact_capturing_rehydration_map(&mut doc, &redaction_config)
        .await
        .map_err(|e| McpError::internal_error(format!("xberg redaction failed: {e}"), None))?;

    let mut findings = doc.redaction_report.map(|report| report.findings).unwrap_or_default();
    let mut rehydration_map: std::collections::BTreeMap<String, String> = map.into_iter().collect();

    // A French company identifier (SIREN, SIRET) is a run of digits the engine files as a
    // phone number. The legal caller acts on the label, so it has to be `tax_id`.
    crate::pii::relabel_in_text(&mut findings, &original);
    let mut redacted_text = std::mem::take(&mut doc.content);
    // …and the token in the text carries that label too: a company identifier is not
    // `[PHONE_3]`.
    crate::pii::rename_tokens(&mut redacted_text, &mut rehydration_map, &findings);

    // The case dictionary: the value becomes its code name, whether the engine found it or
    // not. Fail closed: an entry the caller meant to hide must never be silently dropped —
    // a malformed pair aborts the call rather than leaving the real value standing in the
    // text the agent receives.
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
            crate::pii::Pseudonym::new(entry[0].clone(), entry[1].clone()).ok_or_else(|| {
                McpError::invalid_params(
                    "a pseudonym's value and code name must both be non-empty".to_string(),
                    None,
                )
            })
        })
        .collect::<Result<Vec<_>, McpError>>()?;
    if !dictionary.is_empty() {
        let (substituted, _) = crate::pii::apply_pseudonyms(&redacted_text, &rehydration_map, &dictionary);
        redacted_text = substituted;
    }

    let detections: Vec<serde_json::Value> = findings
        .iter()
        .map(|finding| {
            let start = finding.start as usize;
            let end = finding.end as usize;
            let mut detection = serde_json::json!({
                "category": detection_category(&finding.category),
                "start": start,
                "end": end,
                "text": original.get(start..end).unwrap_or("")
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
#[cfg(feature = "ner-candle")]
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
#[cfg(feature = "ner-candle")]
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

/// The labels every call asks GLiNER2 for, in the order the candle backend
/// will build its schema prompt from — `organization`/`location` as extra
/// zero-shot labels so the three categories redaction claimed before #37 keep
/// working (neither is among the 42), then the 42 PII labels and the AI Act
/// labels. The same list gates the window planner's budget (#71), so the
/// prompt the backend runs with is the prompt the budget was measured for.
#[cfg(feature = "ner-candle")]
fn ner_labels() -> Vec<String> {
    let mut labels = vec!["organization".to_string(), "location".to_string()];
    labels.extend(
        GLI_NER2_PII_LABELS
            .iter()
            .chain(AI_ACT_NER_LABELS.iter())
            .map(|label| (*label).to_string()),
    );
    labels
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
async fn run_ner_windowed(model_dir: Option<&str>, text: &str) -> Result<NerRun, String> {
    use crate::mcp::ner_windowing;
    use std::time::{Duration, Instant};
    use xberg::text::ner::NerBackend;
    use xberg::text::ner::candle::CandleBackend;
    /// Wall-clock budget for the whole NER pass inside one call. Windows run
    /// sequentially, so an extremely large input could otherwise hold an MCP
    /// call for hours (the app measured roughly 4 s per 400-character window
    /// on a CPU-only i5-7360U; 1 MiB would be thousands). Past the deadline
    /// the run reports truncation exactly like a budget-stopped plan:
    /// `require_ner` fails, the pattern pass stands, nothing is cut silently.
    const NER_TIME_BUDGET: Duration = Duration::from_secs(10 * 60);
    let Some(dir) = model_dir else {
        return Err("no ner_model_dir given".to_string());
    };
    let labels = ner_labels();
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
async fn run_ner_windowed(model_dir: Option<&str>, _text: &str) -> Result<NerRun, String> {
    if let Some(dir) = model_dir {
        tracing::warn!(dir, "built without the ner-candle feature; redacting pattern-only");
    }
    Err("built without the ner-candle feature".to_string())
}

#[cfg(test)]
#[path = "redact_file_tests.rs"]
mod redact_file_tests;
