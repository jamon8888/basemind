//! Stable metadata translations and redaction-cache fingerprints for document blobs.

use std::borrow::Cow;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::config::RedactionConfig;

use super::DocLanguageConfidence;

/// What the redaction fingerprint must cover: the config *and* the post-processing the
/// extractor applies to the engine's findings on its own motion (currently the tax_id
/// re-labelling — SIREN/SIRET company identifiers — see [`crate::pii`]).
const REDACTION_PIPELINE_TAG: &str = "redaction-pipeline/3";

/// Deterministic fingerprint of a `RedactionConfig` (+ the pipeline tag) for cache
/// invalidation. Two configs that produce identical redaction behaviour hash to the same
/// value; any field change produces a different fingerprint, forcing re-extraction of
/// cached doc blobs.
pub(crate) fn hash_redaction_config(cfg: &RedactionConfig) -> String {
    let json = serde_json::to_string(&(REDACTION_PIPELINE_TAG, cfg)).unwrap_or_default();
    let mut hasher = DefaultHasher::new();
    json.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

pub(super) fn map_language_confidences(input: Vec<xberg::types::LanguageConfidence>) -> Vec<DocLanguageConfidence> {
    input
        .into_iter()
        .map(|language| DocLanguageConfidence {
            language: language.language,
            confidence: language.confidence,
            proportion: language.proportion,
            script: language.script,
            reliable: language.reliable,
        })
        .collect()
}

/// Stable lowercase tag for xberg's `KeywordAlgorithm`. We avoid `Display`
/// because the enum doesn't derive it; matching every variant keeps the
/// translation explicit and the compiler honest if xberg adds variants.
pub(super) fn keyword_algorithm_str(alg: &xberg::KeywordAlgorithm) -> &'static str {
    match alg {
        xberg::KeywordAlgorithm::Yake => "yake",
        xberg::KeywordAlgorithm::Rake => "rake",
    }
}

/// Flatten xberg's `EntityCategory` (a closed enum with a `Custom(String)`
/// tail variant) to a lowercase string. Standard variants return a `'static`
/// borrow — zero allocation. `Custom(s)` moves `s` into a `Cow::Owned` so
/// callers can call `.into_owned()` without an extra clone for the common case.
pub(super) fn entity_category_str(category: xberg::types::entity::EntityCategory) -> Cow<'static, str> {
    use xberg::types::entity::EntityCategory::*;
    match category {
        Person => Cow::Borrowed("person"),
        Organization => Cow::Borrowed("organization"),
        Location => Cow::Borrowed("location"),
        Date => Cow::Borrowed("date"),
        Time => Cow::Borrowed("time"),
        Money => Cow::Borrowed("money"),
        Percent => Cow::Borrowed("percent"),
        Email => Cow::Borrowed("email"),
        Phone => Cow::Borrowed("phone"),
        Url => Cow::Borrowed("url"),
        Custom(s) => Cow::Owned(s),
    }
}

pub(super) fn metadata_pairs(metadata: &xberg::types::Metadata) -> Vec<(String, String)> {
    match serde_json::to_value(metadata) {
        Ok(serde_json::Value::Object(map)) => map
            .into_iter()
            .filter_map(|(k, v)| {
                let value_str = match v {
                    serde_json::Value::Null => return None,
                    serde_json::Value::String(s) => s,
                    other => other.to_string(),
                };
                Some((k, value_str))
            })
            .collect(),
        _ => Vec::new(),
    }
}
