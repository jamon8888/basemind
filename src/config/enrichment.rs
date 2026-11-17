//! Opt-in document enrichment sub-configs (reranking, keywords).
//!
//! Split from `documents.rs` to keep both files under the 1000-line cap
//! (`tests/max_lines.rs` enforces it): the reranker custom-model support
//! pushed `documents.rs` to 1096 lines. Re-exported through `documents`
//! so every existing `config::…` import path keeps working.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RerankerConfig {
    /// Master switch — off by default; the reranker model download + per-query
    /// latency means users should opt in explicitly.
    #[serde(default)]
    pub enabled: bool,
    /// Xberg reranker preset name. Defaults to `bge-reranker-v2-m3` (multilingual,
    /// 568M params, 100+ languages) per the GDPR PII spec: cross-encoder reranking
    /// must cover all EU languages. Costs nothing while `enabled = false` (default).
    #[serde(default = "RerankerConfig::default_preset")]
    pub preset: String,
    /// How many hits to rerank. The vector search returns `top_k` candidates
    /// which the cross-encoder then reorders. Defaults to 20 per spec.
    #[serde(default = "RerankerConfig::default_top_k")]
    pub top_k: usize,
    /// Custom ONNX cross-encoder (Hugging Face repo) used when no per-call
    /// preset is given. Lets deployments run models xberg ships no preset
    /// for (e.g. `onnx-community/gte-multilingual-reranker-base` int8).
    /// An explicit per-call preset always wins over this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_model: Option<CustomRerankerModel>,
}

/// Custom ONNX cross-encoder resolved from a Hugging Face repo by xberg's
/// lazy downloader (same layout convention as its `Custom` embedding models).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CustomRerankerModel {
    /// Hugging Face repo id, e.g. `"onnx-community/gte-multilingual-reranker-base"`.
    pub model_id: String,
    /// ONNX file within the repo. Defaults to `"onnx/model.onnx"`; int8
    /// variants use e.g. `"onnx/model_int8.onnx"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_file: Option<String>,
}

/// Default ONNX path inside a custom reranker repo (xberg convention).
pub const DEFAULT_CUSTOM_RERANKER_MODEL_FILE: &str = "onnx/model.onnx";

/// Resolved reranker model, in plain data (no xberg types) so this stays
/// compilable without the `intelligence` feature; call sites map it on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedRerankerModel {
    Preset { name: String },
    Custom { model_id: String, model_file: String },
}

impl RerankerConfig {
    fn default_preset() -> String {
        "bge-reranker-v2-m3".to_string()
    }
    fn default_top_k() -> usize {
        20
    }

    /// Resolve which model serves a rerank call. One shared guard: an
    /// explicit per-call preset always means a compiled-in preset; otherwise
    /// the configured custom model wins over the default preset.
    pub fn resolve_model(&self, preset_override: Option<&str>) -> ResolvedRerankerModel {
        match preset_override {
            Some(name) => ResolvedRerankerModel::Preset { name: name.to_string() },
            None => match &self.custom_model {
                Some(custom) => ResolvedRerankerModel::Custom {
                    model_id: custom.model_id.clone(),
                    model_file: custom
                        .model_file
                        .clone()
                        .unwrap_or_else(|| DEFAULT_CUSTOM_RERANKER_MODEL_FILE.to_string()),
                },
                None => ResolvedRerankerModel::Preset {
                    name: self.preset.clone(),
                },
            },
        }
    }
}

impl Default for RerankerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            preset: Self::default_preset(),
            top_k: Self::default_top_k(),
            custom_model: None,
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
