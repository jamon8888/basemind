//! Document-tier sub-configs. Split from `v1.rs` to keep both files under the
//! 1000-line cap once iters 3–7 fill in every xberg capability.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Top-level `[documents]` table. Each sub-config has `#[serde(default)]` so
/// adding a new tier never breaks older TOML files.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocumentsConfig {
    /// Master switch. Only meaningful when the `documents` cargo feature is compiled in.
    #[serde(default = "DocumentsConfig::default_enabled")]
    pub enabled: bool,
    /// MIME-type allowlist. Empty = accept anything xberg can handle.
    #[serde(default)]
    #[schemars(inner(length(min = 1)))]
    pub mime_allowlist: Vec<String>,
    /// Extra file extensions (lowercase, no dot) to skip on top of the built-in archive/binary
    /// denylist. The built-in floor (`.zip/.tar/.jar/.so/.wasm/...`) is always applied — this only
    /// adds to it — so archives and binaries are never routed to xberg extraction + embedding.
    #[serde(default)]
    #[schemars(inner(length(min = 1)))]
    pub extension_denylist: Vec<String>,
    /// Hard cap on chunks embedded per document. A pathological input (huge generated file, an
    /// archive that slipped the denylist) can otherwise explode into tens of thousands of ONNX
    /// embeds. Documents exceeding the cap are extracted but not embedded (blob written, no vector
    /// rows) with a warning.
    #[serde(default = "DocumentsConfig::default_max_chunks_per_document")]
    #[schemars(range(min = 1))]
    pub max_chunks_per_document: usize,
    /// Maximum pages xberg may extract from a single document.
    #[serde(default = "DocumentsConfig::default_max_pages")]
    #[schemars(range(min = 1))]
    pub max_pages: usize,
    /// Maximum wall-clock time xberg may spend extracting one document.
    #[serde(default = "DocumentsConfig::default_extraction_timeout_secs")]
    #[schemars(range(min = 1))]
    pub extraction_timeout_secs: u64,
    /// Maximum chunk size in characters.
    #[serde(default = "DocumentsConfig::default_max_characters")]
    #[schemars(range(min = 64))]
    pub max_characters: usize,
    /// Overlap between chunks in characters.
    #[serde(default = "DocumentsConfig::default_overlap")]
    #[schemars(range(min = 0))]
    pub overlap: usize,
    /// Xberg embedding preset name. Defaults to "balanced".
    #[serde(default = "DocumentsConfig::default_embedding_preset")]
    pub embedding_preset: String,
    /// Generate embeddings (`true`) or skip vector storage entirely (`false`).
    #[serde(default = "DocumentsConfig::default_embed")]
    pub embed: bool,
    /// Glob patterns (repo-relative, forward-slash) for documents that are still extracted +
    /// indexed but **never** embedded. Keeps ONNX vectors off selected large or low-value documents
    /// while leaving their text searchable by keyword. Empty by default; only consulted when
    /// `embed = true`.
    #[serde(default)]
    #[schemars(inner(length(min = 1)))]
    pub embed_exclude: Vec<String>,
    /// Route archives (`.zip/.tar/.jar/...`) into xberg's recursive archive extractor. Default
    /// `false`: archives and compressed containers are rejected by the built-in floor before any
    /// extraction, so a single archive can't explode into thousands of embeds. Flip to `true` to opt
    /// into extracting the files *inside* archives — at the cost of the extra recursion + embedding
    /// work. True binaries (`.so/.wasm/.exe/...`) are always rejected regardless of this flag.
    #[serde(default = "DocumentsConfig::default_extract_archives")]
    pub extract_archives: bool,
    /// Language detection + preferred languages for chunking / extraction.
    #[serde(default)]
    pub language: DocLanguageConfig,
    /// Cross-encoder reranker applied post-vector-search.
    #[serde(default)]
    pub reranker: RerankerConfig,
    /// Keyword extraction at ingest time.
    #[serde(default)]
    pub keywords: KeywordsConfig,
    /// Named-entity recognition at ingest time.
    #[serde(default)]
    pub ner: NerConfig,
    /// Document summarization at ingest time.
    #[serde(default)]
    pub summarization: SummarizationConfig,
    /// PII redaction / anonymisation at ingest time.
    #[serde(default)]
    pub redaction: RedactionConfig,
    /// OCR backend selection for image / scanned-PDF inputs.
    #[serde(default)]
    pub ocr: OcrConfig,
    /// Response wire format for the document MCP tools.
    #[serde(default)]
    pub output: OutputConfig,
    /// Maximum number of threads dedicated to ONNX embedding. `0` means
    /// "auto = max(2, logical_cpus / 4)" — a bounded fraction of available
    /// cores so the embedder never pins the machine. Both the document and
    /// code-search tiers route their `embed_texts` calls through a single
    /// process-wide rayon `ThreadPool` of this size; code-map extraction
    /// keeps the full global pool.
    #[serde(default)]
    pub embed_max_threads: usize,
}

impl DocumentsConfig {
    fn default_enabled() -> bool {
        true
    }
    fn default_max_characters() -> usize {
        1000
    }
    fn default_overlap() -> usize {
        200
    }
    fn default_embedding_preset() -> String {
        "balanced".to_string()
    }
    fn default_embed() -> bool {
        true
    }
    fn default_extract_archives() -> bool {
        false
    }
    fn default_max_chunks_per_document() -> usize {
        2000
    }
    fn default_max_pages() -> usize {
        500
    }
    fn default_extraction_timeout_secs() -> u64 {
        600
    }
}

impl Default for DocumentsConfig {
    fn default() -> Self {
        Self {
            enabled: Self::default_enabled(),
            mime_allowlist: Vec::new(),
            extension_denylist: Vec::new(),
            max_chunks_per_document: Self::default_max_chunks_per_document(),
            max_pages: Self::default_max_pages(),
            extraction_timeout_secs: Self::default_extraction_timeout_secs(),
            max_characters: Self::default_max_characters(),
            overlap: Self::default_overlap(),
            embedding_preset: Self::default_embedding_preset(),
            embed: Self::default_embed(),
            embed_exclude: Vec::new(),
            extract_archives: Self::default_extract_archives(),
            language: DocLanguageConfig::default(),
            reranker: RerankerConfig::default(),
            keywords: KeywordsConfig::default(),
            ner: NerConfig::default(),
            summarization: SummarizationConfig::default(),
            redaction: RedactionConfig::default(),
            ocr: OcrConfig::default(),
            output: OutputConfig::default(),
            embed_max_threads: 0,
        }
    }
}

/// Language detection knobs for the document tier. Named `DocLanguageConfig` to
/// avoid colliding with the per-tree-sitter-language `LanguageConfig` (which is
/// the scanner's per-grammar override map).
///
/// Xberg drives language detection through the `whatlang` crate and reports
/// ISO 639-3 codes (three letters, e.g. `"fra"`, `"deu"`) — xberg's own
/// `ExtractionResult.detected_languages` doc-comment mislabels them as ISO
/// 639-1 in rc.10, but the wrapper at `xberg::language_detection` normalises
/// every `whatlang` enum variant to its ISO 639-3 form. The `auto_detect`,
/// `min_confidence`, and `detect_multiple` knobs map straight through to
/// xberg's `LanguageDetectionConfig`. `preferred_languages` is reserved for
/// future use — xberg rc.10 does not honor a preferred-language hint, so
/// the field is plumbed but inert today; we keep it on the schema so callers
/// can start populating it without a config break when support lands.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocLanguageConfig {
    /// Run xberg's language detector. On by default; flip off when every doc
    /// in the corpus is the same known language.
    #[serde(default = "DocLanguageConfig::default_auto_detect")]
    pub auto_detect: bool,
    /// Minimum detector confidence (0.0–1.0). Detections below this threshold
    /// are dropped. Matches xberg's default of 0.8.
    #[serde(default = "DocLanguageConfig::default_min_confidence")]
    #[schemars(range(min = 0.0, max = 1.0))]
    pub min_confidence: f64,
    /// When true, xberg reports every language detected in the document
    /// instead of just the top match. Off by default to match xberg.
    #[serde(default)]
    pub detect_multiple: bool,
    /// Reserved — accepts ISO 639-3 codes (e.g. `"fra"`, `"deu"`) for future use.
    /// Xberg rc.10 does not honor a preferred-language hint, but the field
    /// is kept on the schema so users can populate it without a config break.
    #[serde(default)]
    pub preferred_languages: Vec<String>,
}

impl DocLanguageConfig {
    fn default_auto_detect() -> bool {
        true
    }
    fn default_min_confidence() -> f64 {
        0.8
    }
}

impl Default for DocLanguageConfig {
    fn default() -> Self {
        Self {
            auto_detect: Self::default_auto_detect(),
            min_confidence: Self::default_min_confidence(),
            detect_multiple: false,
            preferred_languages: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RerankerConfig {
    /// Master switch — off by default; the reranker model download + per-query
    /// latency means users should opt in explicitly.
    #[serde(default)]
    pub enabled: bool,
    /// Xberg reranker preset name (`bge-reranker-base` is the small default;
    /// `bge-reranker-large` and `bge-reranker-v2-m3` are heavier alternatives).
    #[serde(default = "RerankerConfig::default_preset")]
    pub preset: String,
    /// How many hits to rerank. The vector search returns `top_k` candidates
    /// which the cross-encoder then reorders.
    #[serde(default = "RerankerConfig::default_top_k")]
    pub top_k: usize,
}

impl RerankerConfig {
    fn default_preset() -> String {
        "bge-reranker-base".to_string()
    }
    fn default_top_k() -> usize {
        10
    }
}

impl Default for RerankerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            preset: Self::default_preset(),
            top_k: Self::default_top_k(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KeywordsConfig {
    /// Master switch — off by default; YAKE / RAKE add ingest-time CPU cost.
    /// Maps to `Some(KeywordConfig)` / `None` on `ExtractionConfig.keywords`;
    /// xberg's own `KeywordConfig` has no `enabled` field — gating is via
    /// the wrapping `Option`.
    #[serde(default)]
    pub enabled: bool,
    /// Algorithm: YAKE (statistical, multi-language) or RAKE (rapid automatic
    /// keyword extraction).
    #[serde(default)]
    pub algorithm: KeywordAlgorithm,
    /// Maximum keywords to extract per document. Matches xberg's
    /// `KeywordConfig.max_keywords` default of 10.
    #[serde(default = "KeywordsConfig::default_max_keywords")]
    pub max_keywords: usize,
    /// Minimum score threshold. Matches xberg's `KeywordConfig.min_score`
    /// default of 0.0 (i.e. surface every candidate). Score ranges differ
    /// between YAKE (lower = better) and RAKE (higher = better) — see
    /// `xberg::keywords::config::KeywordConfig.min_score`.
    #[serde(default)]
    #[schemars(range(min = 0.0))]
    pub min_score: f32,
    /// N-gram range as `[min, max]`. Matches xberg's
    /// `KeywordConfig.ngram_range` default of `(1, 3)`. Encoded as an array of
    /// length 2 so the JSON Schema stays human-readable; values map back to a
    /// `(usize, usize)` tuple at the boundary.
    #[serde(default = "KeywordsConfig::default_ngram_range")]
    #[schemars(length(min = 2, max = 2))]
    pub ngram_range: Vec<usize>,
    /// Optional YAKE tuning (passed through to xberg unchanged). Shape
    /// matches `xberg::keywords::YakeParams`; bad JSON is logged and
    /// xberg's defaults are used instead of failing the scan.
    #[serde(default)]
    pub yake_params: Option<serde_json::Value>,
    /// Optional RAKE tuning (passed through to xberg unchanged). Shape
    /// matches `xberg::keywords::RakeParams`; bad JSON is logged and
    /// xberg's defaults are used instead of failing the scan.
    #[serde(default)]
    pub rake_params: Option<serde_json::Value>,
}

impl KeywordsConfig {
    fn default_max_keywords() -> usize {
        10
    }
    fn default_ngram_range() -> Vec<usize> {
        vec![1, 3]
    }
}

impl Default for KeywordsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            algorithm: KeywordAlgorithm::default(),
            max_keywords: Self::default_max_keywords(),
            min_score: 0.0,
            ngram_range: Self::default_ngram_range(),
            yake_params: None,
            rake_params: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "lowercase")]
pub enum KeywordAlgorithm {
    #[default]
    Yake,
    Rake,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NerConfig {
    /// Master switch — off by default; ONNX backend downloads gline-rs weights on
    /// first use, LLM backend costs API tokens. Maps to `Some(NerConfig)` /
    /// `None` on `ExtractionConfig.ner`.
    #[serde(default)]
    pub enabled: bool,
    /// Backend selection. `onnx` uses gline-rs locally; `llm` routes through the
    /// shared `[llm]` config.
    #[serde(default)]
    pub backend: NerBackend,
    /// Override the ONNX model name (xberg has a default catalogue when unset).
    #[serde(default)]
    pub model: Option<String>,
    /// Categories to surface — matches xberg's `NerConfig.categories`.
    /// Accepted values are the lowercase forms `"person"`, `"organization"`,
    /// `"location"`, `"date"`, `"time"`, `"money"`, `"percent"`, `"email"`,
    /// `"phone"`, `"url"`; anything else becomes a `Custom(_)` category at the
    /// boundary. Empty means "use the backend default".
    #[serde(default)]
    pub categories: Vec<String>,
    /// Arbitrary user-supplied entity labels for gline-rs zero-shot inference
    /// (and the LLM backend's structured-output schema). Matches xberg's
    /// `NerConfig.custom_labels`. Useful for domain-specific types like
    /// `"Treatment"`, `"Vessel"` without forking GLiNER's taxonomy. Custom
    /// labels surface as `EntityCategory::Custom(_)` in the entity stream.
    #[serde(default)]
    pub custom_labels: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "lowercase")]
pub enum NerBackend {
    #[default]
    Onnx,
    Llm,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SummarizationConfig {
    /// Master switch — off by default. When true, the boundary code populates
    /// `ExtractionConfig.summarization`; the resulting `ExtractionResult.summary`
    /// surfaces on `FileMapDoc.summary` and `DocumentSearchHit.summary`.
    #[serde(default)]
    pub enabled: bool,
    /// Strategy: `extractive` (TextRank, no LLM) or `abstractive` (routed via
    /// the top-level `[llm]` config). When `abstractive` is set but `[llm].model`
    /// is empty, the boundary falls back to `extractive` with a warning so the
    /// scan still completes.
    #[serde(default)]
    pub strategy: SummarizationStrategy,
    /// Soft cap on summary length in tokens. `None` lets xberg pick a default
    /// suited to the chosen strategy (the xberg-side type uses the same
    /// `Option<u32>` shape; pass-through with no policy of our own).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "lowercase")]
pub enum SummarizationStrategy {
    #[default]
    Extractive,
    Abstractive,
}

/// PII redaction / anonymisation config.
///
/// When `enabled = true`, xberg's redaction post-processor runs at the Late stage
/// of document extraction and rewrites every textual field (content, tables, metadata,
/// URIs, form fields, OCR, Djot, etc.) with `[TYPE_N]` tokens via `TokenReplace`.
/// The rehydration map is NOT stored by basemind — it is returned to the caller
/// via `ExtractedDocument.redaction_report`. Use `basemind vault-encrypt` to
/// encrypt and persist the map separately.
///
/// Requires `xberg/redaction` (enabled in the `documents` feature) and
/// `xberg/redaction-rehydrate` (also enabled in the `documents` feature) for the
/// `TokenReplace` strategy that enables later rehydration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RedactionConfig {
    /// Master switch — off by default.
    #[serde(default)]
    pub enabled: bool,
    /// PII categories to redact. Empty = all categories supported by the engine.
    /// Accepted values: `email`, `phone`, `ssn`, `credit_card`, `iban`,
    /// `ip_address`, `swift_bic`, `postal_code`, `person`, `organization`,
    /// `location`, `date_of_birth`. Categories suffixed with `_strict` use
    /// checksum validation where applicable (e.g. `iban_strict`).
    #[serde(default)]
    pub categories: Vec<String>,
    /// Redaction strategy. `token_replace` (default) assigns stable `[TYPE_N]`
    /// tokens per category and is reversible via the rehydration map. `mask`
    /// replaces with `[REDACTED]` irreversibly. `hash` replaces with
    /// `[HASH:...` irreversibly. `drop` removes the span with no marker.
    #[serde(default)]
    pub strategy: RedactionStrategy,
    /// NER backend config for PERSON / ORGANIZATION / LOCATION categories.
    #[serde(default)]
    pub ner: Option<NerRedactionConfig>,
    /// Preserve chunk byte ranges after redaction (default: `true`).
    #[serde(default = "default_true")]
    pub preserve_offsets: bool,
    /// Custom literal terms to redact. Each hit surfaces as `Custom(<label>)`
    /// in the redaction report.
    #[serde(default)]
    pub custom_terms: Vec<RedactionCustomTerm>,
    /// Custom regex patterns to redact. Each hit surfaces as `Custom(<label>)`
    /// in the redaction report.
    #[serde(default)]
    pub custom_patterns: Vec<RedactionCustomPattern>,
}

impl Default for RedactionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            categories: Vec::new(),
            strategy: RedactionStrategy::TokenReplace,
            ner: None,
            preserve_offsets: true,
            custom_terms: Vec::new(),
            custom_patterns: Vec::new(),
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "kebab-case")]
pub enum RedactionStrategy {
    #[default]
    TokenReplace,
    Mask,
    Hash,
    Drop,
}

/// NER backend config for redaction (separate from the extraction-tier NER config).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NerRedactionConfig {
    /// NER backend: `onnx` (local GLiNER) or `llm` (provider API).
    #[serde(default)]
    pub backend: NerBackend,
    /// Override the ONNX model name.
    #[serde(default)]
    pub model: Option<String>,
    /// Categories to detect via NER.
    #[serde(default)]
    pub categories: Vec<String>,
    /// Custom labels for zero-shot detection.
    #[serde(default)]
    pub custom_labels: Vec<String>,
}

impl Default for NerRedactionConfig {
    fn default() -> Self {
        Self {
            backend: NerBackend::default(),
            model: None,
            categories: Vec::new(),
            custom_labels: Vec::new(),
        }
    }
}

/// A literal string to redact, surfaced as `Custom(<label>)` in the report.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RedactionCustomTerm {
    /// Label shown in the redaction report for each hit.
    pub label: String,
    /// Literal value to find and redact.
    pub value: String,
    /// Case-sensitive match (default: false, i.e. case-insensitive).
    #[serde(default)]
    pub case_sensitive: bool,
}

/// A regex pattern to redact, surfaced as `Custom(<label>)` in the report.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RedactionCustomPattern {
    /// Label shown in the redaction report for each hit.
    pub label: String,
    /// Regex pattern (Rust dialect, no look-around).
    pub pattern: String,
    /// Case-sensitive match (default: false, i.e. case-insensitive).
    #[serde(default)]
    pub case_sensitive: bool,
}

#[cfg(feature = "intelligence")]
impl RedactionConfig {
    /// Translate the basemind-side `RedactionConfig` into xberg's `RedactionConfig`.
    /// Returns `None` when redaction is disabled.
    pub fn to_xberg(&self) -> Option<xberg::RedactionConfig> {
        if !self.enabled {
            return None;
        }

        let categories = if self.categories.is_empty() {
            Vec::new()
        } else {
            self.categories
                .iter()
                .filter_map(|c| match c.to_lowercase().as_str() {
                    "email" => Some(xberg::types::redaction::PiiCategory::Email),
                    "phone" => Some(xberg::types::redaction::PiiCategory::Phone),
                    "ssn" => Some(xberg::types::redaction::PiiCategory::Ssn),
                    "credit_card" => Some(xberg::types::redaction::PiiCategory::CreditCard),
                    "iban" => Some(xberg::types::redaction::PiiCategory::Iban),
                    "ip_address" => Some(xberg::types::redaction::PiiCategory::IpAddress),
                    "swift_bic" => Some(xberg::types::redaction::PiiCategory::SwiftBic),
                    "postal_code" => Some(xberg::types::redaction::PiiCategory::PostalCode),
                    "date_of_birth" => Some(xberg::types::redaction::PiiCategory::DateOfBirth),
                    "person" => Some(xberg::types::redaction::PiiCategory::Person),
                    "organization" => Some(xberg::types::redaction::PiiCategory::Organization),
                    "location" => Some(xberg::types::redaction::PiiCategory::Location),
                    s => Some(xberg::types::redaction::PiiCategory::Custom(s.into())),
                })
                .collect()
        };

        let strategy = match self.strategy {
            RedactionStrategy::TokenReplace => xberg::types::redaction::RedactionStrategy::TokenReplace,
            RedactionStrategy::Mask => xberg::types::redaction::RedactionStrategy::Mask,
            RedactionStrategy::Hash => xberg::types::redaction::RedactionStrategy::Hash,
            RedactionStrategy::Drop => xberg::types::redaction::RedactionStrategy::Drop,
        };

        let ner = self.ner.as_ref().map(|n| xberg::core::config::ner::NerConfig {
            backend: match n.backend {
                super::NerBackend::Onnx => xberg::core::config::ner::NerBackendKind::Onnx,
                super::NerBackend::Llm => xberg::core::config::ner::NerBackendKind::Llm,
            },
            categories: n
                .categories
                .iter()
                .cloned()
                .map(xberg::types::entity::EntityCategory::from)
                .collect(),
            model: n.model.clone(),
            llm: None,
            custom_labels: n.custom_labels.clone(),
        });

        let custom_terms = self
            .custom_terms
            .iter()
            .map(|t| xberg::RedactionTerm {
                label: t.label.clone(),
                value: t.value.clone(),
                case_sensitive: t.case_sensitive,
            })
            .collect();

        let custom_patterns = self
            .custom_patterns
            .iter()
            .map(|p| xberg::RedactionPattern {
                label: p.label.clone(),
                pattern: p.pattern.clone(),
                case_sensitive: p.case_sensitive,
            })
            .collect();

        Some(xberg::RedactionConfig {
            categories: std::collections::HashSet::from_iter(categories),
            strategy,
            ner,
            preserve_offsets: self.preserve_offsets,
            custom_terms,
            custom_patterns,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OcrConfig {
    /// Backend selection. `tesseract` is the default; `paddle` for CJK-heavy
    /// corpora; `vlm` routes through `[llm]` for vision-language OCR.
    #[serde(default)]
    pub backend: OcrBackend,
    /// Tesseract / PaddleOCR language packs (ISO 639-3 codes like `"eng"`).
    #[serde(default = "OcrConfig::default_languages")]
    pub languages: Vec<String>,
}

impl OcrConfig {
    fn default_languages() -> Vec<String> {
        vec!["eng".to_string()]
    }
}

impl Default for OcrConfig {
    fn default() -> Self {
        Self {
            backend: OcrBackend::default(),
            languages: Self::default_languages(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "lowercase")]
pub enum OcrBackend {
    #[default]
    Tesseract,
    Paddle,
    Vlm,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OutputConfig {
    /// Wire format used by document-tier MCP responses.
    #[serde(default)]
    pub format: OutputFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    #[default]
    Json,
    Toon,
}

/// Shared LLM credentials + model selection. Consumed by every LLM-backed
/// capability (ner-llm, summarization-llm, reranker-llm, VLM OCR).
///
/// Mirrors xberg's `LlmConfig` field-for-field with one safety upgrade:
/// `api_key` is the [`ApiKey`] tri-state (literal / env-ref / unset) instead of
/// `Option<String>`, so credentials are never stored as bare literals in TOML.
/// At the boundary the env-ref is resolved into a [`SecretString`] and then
/// exposed via [`SecretString::expose`] when constructing xberg's struct.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct LlmConfig {
    /// Combined provider/model string following liter-llm's routing format:
    /// `"openai/gpt-4o"`, `"anthropic/claude-sonnet-4-20250514"`,
    /// `"groq/llama-3.1-70b-versatile"`. Empty string ⇒ inert; every LLM-backed
    /// feature treats an empty `model` as "no LLM configured".
    #[serde(default)]
    pub model: String,
    /// API key — either a literal (discouraged; keeps secrets in version control)
    /// or an `{ env = "OPENAI_API_KEY" }` reference. `Unset` lets the underlying
    /// provider SDK fall back to its standard environment variable lookup.
    #[serde(default)]
    pub api_key: ApiKey,
    /// Override the provider base URL (for self-hosted vLLM, Azure OpenAI, …).
    #[serde(default)]
    pub base_url: Option<String>,
    /// Sampling temperature. Provider-default when unset.
    #[serde(default)]
    pub temperature: Option<f64>,
    /// Request timeout in seconds. Maps to xberg's `timeout_secs` (default 60).
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    /// Maximum retry attempts on transient errors. Maps to xberg's
    /// `max_retries` (default 3).
    #[serde(default)]
    pub max_retries: Option<u32>,
    /// Maximum tokens to generate. Provider-default when unset.
    #[serde(default)]
    pub max_tokens: Option<u64>,
}

#[cfg(feature = "intelligence")]
impl LlmConfig {
    /// Translate the basemind-side `LlmConfig` into xberg's `LlmConfig`.
    ///
    /// Returns `None` when `model` is empty — every LLM-backed feature treats
    /// `None` as "no LLM configured" and falls back to the non-LLM path. The
    /// `ApiKey` enum is resolved here: `Unset` and missing env vars become
    /// `None` (letting the provider SDK fall back to its own env-var lookup),
    /// `Literal` and resolved env-refs are exposed via [`SecretString::expose`].
    pub fn to_xberg(&self) -> Option<xberg::LlmConfig> {
        if self.model.is_empty() {
            return None;
        }
        Some(xberg::LlmConfig {
            model: self.model.clone(),
            api_key: self.api_key.resolve().map(|s| s.expose().to_string()),
            base_url: self.base_url.clone(),
            timeout_secs: self.timeout_secs,
            max_retries: self.max_retries,
            temperature: self.temperature,
            max_tokens: self.max_tokens,
            // `None` preserves the prior behavior: basemind resolves the api key itself, ~keep
            // adds no custom provider headers, and leaves xberg's env handling default. ~keep
            load_env: None,
            headers: None,
            ..Default::default()
        })
    }
}

/// Tri-state API key wrapper. Serde-deserialised as `null` (`Unset`), a plain
/// string (`Literal`), or a `{ env = "NAME" }` table (`Env`).
///
/// `Serialize` is implemented manually — the `Literal` variant NEVER emits its
/// cleartext secret. It serialises to the marker string `"<redacted>"` so any
/// downstream pipeline that round-trips this enum through JSON / TOML (e.g.
/// the config validator at `src/config/mod.rs`) cannot leak credentials into
/// logs, snapshot tests, or error messages. Deserialisation is unchanged:
/// loading a TOML config with a literal `api_key = "sk-..."` still produces
/// `ApiKey::Literal(...)`. The redaction is one-way by design.
#[derive(Debug, Clone, Deserialize, JsonSchema, Default)]
#[serde(untagged)]
pub enum ApiKey {
    /// Literal value baked into config. Strongly discouraged for production —
    /// `Env` keeps secrets out of source control.
    Literal(String),
    /// `{ env = "OPENAI_API_KEY" }` — resolved at load time via `std::env::var`.
    Env { env: String },
    /// Missing / `null` — every LLM-backed feature treats this as "disabled".
    #[default]
    Unset,
}

impl PartialEq for ApiKey {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Literal(a), Self::Literal(b)) => a == b,
            (Self::Env { env: a }, Self::Env { env: b }) => a == b,
            (Self::Unset, Self::Unset) => true,
            _ => false,
        }
    }
}

impl serde::Serialize for ApiKey {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Literal(_) => serializer.serialize_str("<redacted>"),
            Self::Env { env } => {
                use serde::ser::SerializeStruct;
                let mut s = serializer.serialize_struct("EnvRef", 1)?;
                s.serialize_field("env", env)?;
                s.end()
            }
            Self::Unset => serializer.serialize_none(),
        }
    }
}

impl ApiKey {
    /// Resolve the credential. `Literal` returns its inner value as-is; `Env`
    /// performs an `env::var` lookup and returns `None` if the variable is
    /// missing or empty; `Unset` always returns `None`.
    pub fn resolve(&self) -> Option<SecretString> {
        match self {
            Self::Literal(s) if !s.is_empty() => Some(SecretString(s.clone())),
            Self::Literal(_) => None,
            Self::Env { env } => match std::env::var(env) {
                Ok(v) if !v.is_empty() => Some(SecretString(v)),
                _ => None,
            },
            Self::Unset => None,
        }
    }
}

/// Newtype wrapping a resolved secret. `Debug` and `Display` always render
/// `"<redacted>"` so spans / panics never leak the value. Use `expose()` when
/// you genuinely need the raw bytes at an API boundary.
#[derive(Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SecretString(String);

impl SecretString {
    /// Wrap a raw string as a secret. The caller is responsible for ensuring
    /// the bytes are not already on disk in plaintext.
    pub fn new(value: String) -> Self {
        Self(value)
    }

    /// Borrow the underlying secret. Use only at the boundary that consumes the
    /// credential (e.g. building an HTTP authorization header).
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("\"<redacted>\"")
    }
}

impl fmt::Display for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_string_redacts_in_debug() {
        let s = SecretString::new("hunter2".to_string());
        assert_eq!(format!("{s:?}"), "\"<redacted>\"");
        assert_eq!(format!("{s}"), "<redacted>");
        assert_eq!(s.expose(), "hunter2");
    }

    #[test]
    fn api_key_unset_resolves_to_none() {
        assert!(ApiKey::Unset.resolve().is_none());
    }

    #[test]
    fn api_key_literal_resolves_to_value() {
        let k = ApiKey::Literal("sk-test".to_string());
        let resolved = k.resolve().expect("literal resolves");
        assert_eq!(resolved.expose(), "sk-test");
    }

    #[test]
    fn api_key_literal_empty_resolves_to_none() {
        assert!(ApiKey::Literal(String::new()).resolve().is_none());
    }

    #[test]
    fn api_key_env_reads_environment() {
        // SAFETY: scoped to this single test; the env var name is uniquely
        unsafe {
            std::env::set_var("BASEMIND_TEST_API_KEY_PRESENT", "value-123");
        }
        let k = ApiKey::Env {
            env: "BASEMIND_TEST_API_KEY_PRESENT".to_string(),
        };
        let resolved = k.resolve().expect("env resolves");
        assert_eq!(resolved.expose(), "value-123");
        unsafe {
            std::env::remove_var("BASEMIND_TEST_API_KEY_PRESENT");
        }
    }

    #[test]
    fn api_key_env_missing_resolves_to_none() {
        // SAFETY: the variable is removed before the test so the env lookup
        unsafe {
            std::env::remove_var("BASEMIND_TEST_API_KEY_MISSING");
        }
        let k = ApiKey::Env {
            env: "BASEMIND_TEST_API_KEY_MISSING".to_string(),
        };
        assert!(k.resolve().is_none());
    }

    #[test]
    fn api_key_deserialises_literal_string() {
        let k: ApiKey = serde_json::from_str("\"sk-test\"").expect("parse");
        match k {
            ApiKey::Literal(s) => assert_eq!(s, "sk-test"),
            other => panic!("expected Literal, got {other:?}"),
        }
    }

    #[test]
    fn api_key_deserialises_env_table() {
        let k: ApiKey = serde_json::from_str(r#"{"env":"OPENAI_API_KEY"}"#).expect("parse");
        match k {
            ApiKey::Env { env } => assert_eq!(env, "OPENAI_API_KEY"),
            other => panic!("expected Env, got {other:?}"),
        }
    }

    #[test]
    fn api_key_literal_never_serializes_cleartext() {
        let key = ApiKey::Literal("sk-supersecret".to_string());
        let json = serde_json::to_string(&key).expect("serialize");
        assert!(!json.contains("sk-supersecret"), "raw secret leaked: {json}");
        assert!(json.contains("<redacted>"), "redaction marker missing: {json}");
    }

    #[test]
    fn api_key_env_serializes_env_name_only() {
        let key = ApiKey::Env {
            env: "OPENAI_API_KEY".to_string(),
        };
        let json = serde_json::to_string(&key).expect("serialize");
        assert!(json.contains("OPENAI_API_KEY"), "env name missing: {json}");
        assert!(json.contains("\"env\""), "env field missing: {json}");
    }

    #[test]
    fn api_key_unset_serializes_to_null() {
        let key = ApiKey::Unset;
        let json = serde_json::to_string(&key).expect("serialize");
        assert_eq!(json, "null");
    }
}
