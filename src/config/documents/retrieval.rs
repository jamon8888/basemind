//! Retrieval-time configuration for the documents tier: index build and query path.
//!
//! Split out of `src/config/documents.rs` by the 1000-line module cap (`tests/max_lines.rs`), and
//! because the three tables below are one concern — how the FTS index is built and how hits are
//! ranked — rather than three separate ones.
//!
//! Nothing here has a `deny_unknown_fields` problem: every field carries `#[serde(default)]`, per
//! the convention in the parent module, so an older `basemind.toml` keeps loading.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `[documents.fusion]` — RRF parameters for multi-lane retrieval.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FusionConfig {
    /// RRF rank-damping constant. 60 is the Cormack et al. value and the de-facto default: large
    /// enough that a lane's top ranks stay close in contribution, so no lane wins on rank-1 alone.
    #[serde(default = "FusionConfig::default_k")]
    pub k: f32,

    /// Weight for the `exact` lane (identifier / trigram match over `cites`).
    #[serde(default = "FusionConfig::default_weight_exact")]
    pub weight_exact: f32,

    /// Weight for the `keyword` lane (BM25/FTS over `text` + `heading_path`).
    #[serde(default = "FusionConfig::default_weight_keyword")]
    pub weight_keyword: f32,

    /// Weight for the `facet` lane. A facet lane prefilters without scoring, so this scales a rank
    /// over the surviving set, not a matched score.
    #[serde(default = "FusionConfig::default_weight_facet")]
    pub weight_facet: f32,

    /// Weight for the `vector` lane.
    #[serde(default = "FusionConfig::default_weight_vector")]
    pub weight_vector: f32,

    /// Maximum chunks kept from any one document in a fused result. A long filing otherwise
    /// contributes several near-identical passages that crowd out other documents.
    #[serde(default = "FusionConfig::default_per_document_cap")]
    pub per_document_cap: u32,

    /// How many times a multi-term query may drop its lowest-idf term before giving up.
    ///
    /// This bounds latency on a query that matches nothing conjunctively. Each relaxation is a
    /// whole re-run of the FTS query, so this is a latency knob as much as a recall one.
    #[serde(default = "FusionConfig::default_max_relaxations")]
    pub max_relaxations: u32,
}

impl FusionConfig {
    fn default_k() -> f32 {
        60.0
    }
    fn default_weight_exact() -> f32 {
        3.0
    }
    fn default_weight_keyword() -> f32 {
        2.0
    }
    fn default_weight_facet() -> f32 {
        1.0
    }
    fn default_weight_vector() -> f32 {
        1.0
    }
    fn default_per_document_cap() -> u32 {
        3
    }
    fn default_max_relaxations() -> u32 {
        3
    }
}

impl Default for FusionConfig {
    fn default() -> Self {
        Self {
            k: Self::default_k(),
            weight_exact: Self::default_weight_exact(),
            weight_keyword: Self::default_weight_keyword(),
            weight_facet: Self::default_weight_facet(),
            weight_vector: Self::default_weight_vector(),
            per_document_cap: Self::default_per_document_cap(),
            max_relaxations: Self::default_max_relaxations(),
        }
    }
}

/// `[documents.fts]` — tokenizer settings for the lexical index.
///
/// Every field maps to a real `InvertedIndexParams` field; nothing here is a basemind-level
/// setting masquerading as a tokenizer one. Deliberately absent:
///
/// - **`enabled`.** `[documents].enabled` is already the tier's master switch; a nested one
///   would shadow it and make "the tier is on but the index is off" expressible.
/// - **Cadence keys.** `optimize()` frequency and the unindexed-row warning threshold are build
///   settings, not tokenizer parameters. They do not belong in a table named `fts`.
/// - **`language`.** `[documents].language` already exists and is a *detection* table
///   (`auto_detect`, `min_confidence`, `detect_multiple`, `preferred_languages`). Two different
///   `language` keys of different shapes at different depths is a trap, so the stemmer's language
///   is named for what it does.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FtsConfig {
    /// N-gram length for the citation (trigram) index. Both bounds are set because Lance's n-gram
    /// tokenizer takes a min and a max; 3/3 is what makes `362` reach `§ 362(a)(1)`.
    #[serde(default = "FtsConfig::default_ngram_min_length")]
    pub ngram_min_length: u32,
    /// Upper n-gram length for the citation index. See `ngram_min_length`.
    #[serde(default = "FtsConfig::default_ngram_max_length")]
    pub ngram_max_length: u32,
    /// Stemming on the lexical index.
    #[serde(default = "FtsConfig::default_stem")]
    pub stem: bool,
    /// Stop-word removal on the lexical index. The citation index overrides this to `false` —
    /// removing stop words from a statute reference changes what it means.
    #[serde(default = "FtsConfig::default_remove_stop_words")]
    pub remove_stop_words: bool,
    /// ASCII folding, so `société` matches `societe`. Matters for the French and Spanish corpora
    /// this tier exists for.
    #[serde(default = "FtsConfig::default_ascii_folding")]
    pub ascii_folding: bool,
    /// Maximum token length. Lance's default of 40 truncates long French and Spanish tokens, and a
    /// truncated token indexes as the wrong term.
    #[serde(default = "FtsConfig::default_max_token_length")]
    pub max_token_length: u32,
    /// Language whose stemmer and stop-word list apply.
    #[serde(default = "FtsConfig::default_stemmer_language")]
    pub stemmer_language: String,
    /// Legal boilerplate added to the stop-word list. These carry no retrieval signal and are
    /// frequent enough to dominate a short query.
    #[serde(default = "FtsConfig::default_custom_stop_words")]
    pub custom_stop_words: Vec<String>,
}

impl FtsConfig {
    fn default_ngram_min_length() -> u32 {
        3
    }
    fn default_ngram_max_length() -> u32 {
        3
    }
    fn default_stem() -> bool {
        true
    }
    fn default_remove_stop_words() -> bool {
        true
    }
    fn default_ascii_folding() -> bool {
        true
    }
    fn default_max_token_length() -> u32 {
        64
    }
    fn default_stemmer_language() -> String {
        "English".to_string()
    }
    fn default_custom_stop_words() -> Vec<String> {
        [
            "pursuant",
            "herein",
            "hereto",
            "aforesaid",
            "wherein",
            "notwithstanding",
            "provided that",
            "hereinafter",
            "thereunder",
        ]
        .into_iter()
        .map(str::to_string)
        .collect()
    }
}

impl Default for FtsConfig {
    fn default() -> Self {
        Self {
            ngram_min_length: Self::default_ngram_min_length(),
            ngram_max_length: Self::default_ngram_max_length(),
            stem: Self::default_stem(),
            remove_stop_words: Self::default_remove_stop_words(),
            ascii_folding: Self::default_ascii_folding(),
            max_token_length: Self::default_max_token_length(),
            stemmer_language: Self::default_stemmer_language(),
            custom_stop_words: Self::default_custom_stop_words(),
        }
    }
}

/// `[documents.citations]` — identifier extraction for the exact lane.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CitationsConfig {
    /// Extract normalized identifiers (statute cites, docket numbers, case short names) at ingest.
    ///
    /// The escape hatch for store size: the trigram index over this column is the one most likely
    /// to dominate the store on a large corpus. Turning it off disables the `exact` lane, not the
    /// rest of retrieval.
    #[serde(default = "CitationsConfig::default_extract")]
    pub extract: bool,
    /// Cap on identifiers stored per chunk. A dense statute-heavy chunk can yield hundreds.
    #[serde(default = "CitationsConfig::default_max_per_chunk")]
    pub max_per_chunk: u32,
}

impl CitationsConfig {
    fn default_extract() -> bool {
        true
    }
    fn default_max_per_chunk() -> u32 {
        64
    }
}

impl Default for CitationsConfig {
    fn default() -> Self {
        Self {
            extract: Self::default_extract(),
            max_per_chunk: Self::default_max_per_chunk(),
        }
    }
}

#[cfg(test)]
mod tests {
    // `super` is this file's own re-export set; `DocumentsConfig` lives one level up, in
    // `config::documents`.
    use super::super::DocumentsConfig;
    use super::{CitationsConfig, FtsConfig, FusionConfig};

    /// The defaults must match the spec's §5 / §7.1 / §10 tables, because they are what a user gets
    /// with no config at all. If these drift, the documented defaults are a lie.
    #[test]
    fn defaults_match_the_spec() {
        let fusion = FusionConfig::default();
        assert_eq!(fusion.k, 60.0);
        assert_eq!(
            (
                fusion.weight_exact,
                fusion.weight_keyword,
                fusion.weight_facet,
                fusion.weight_vector
            ),
            (3.0, 2.0, 1.0, 1.0)
        );
        assert_eq!(fusion.per_document_cap, 3);
        assert_eq!(fusion.max_relaxations, 3);

        let fts = FtsConfig::default();
        assert_eq!((fts.ngram_min_length, fts.ngram_max_length), (3, 3));
        assert!(fts.stem && fts.remove_stop_words && fts.ascii_folding);
        assert_eq!(
            fts.max_token_length, 64,
            "Lance's default of 40 truncates long FR/ES tokens"
        );
        assert_eq!(fts.stemmer_language, "English");
        assert!(fts.custom_stop_words.contains(&"aforesaid".to_string()));

        let citations = CitationsConfig::default();
        assert!(citations.extract);
        assert_eq!(citations.max_per_chunk, 64);
    }

    /// Every field has a `#[serde(default)]`, so an older `basemind.toml` — one with no
    /// `[documents.fts]` at all — still parses. Without this the daemon rejects the config outright,
    /// because `DocumentsConfig` is `deny_unknown_fields`.
    #[test]
    fn an_older_config_without_these_tables_still_parses() {
        let cfg: DocumentsConfig = toml::from_str(
            r#"
enabled = true
max_characters = 1800
"#,
        )
        .expect("a config predating the retrieval tables must still load");
        assert!(cfg.enabled);
        assert_eq!(cfg.max_characters, 1800);
        assert_eq!(cfg.fts.max_token_length, 64, "and take the documented defaults");
        assert_eq!(cfg.fusion.k, 60.0);
        assert!(cfg.citations.extract);
    }

    /// Each table rejects keys it does not own, rather than silently ignoring a typo'd setting the
    /// user believes is in effect.
    #[test]
    fn unknown_keys_in_a_retrieval_table_are_rejected() {
        let err = toml::from_str::<FtsConfig>("stemmmer_language = \"French\"").unwrap_err();
        assert!(
            err.to_string().contains("stemmmer"),
            "error names the offending key: {err}"
        );
    }

    /// `language` must not reappear at the FTS level. `[documents].language` is a detection table;
    /// a second key of that name with a different shape is the trap the rename avoids.
    #[test]
    fn fts_config_has_no_bare_language_key() {
        let err = toml::from_str::<FtsConfig>("language = \"French\"").unwrap_err();
        assert!(
            err.to_string().contains("language"),
            "a bare `language` is rejected: {err}"
        );
    }
}
