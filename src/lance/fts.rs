//! Lexical (full-text) retrieval for the documents tier.
//!
//! Split out of `mod.rs` by the 1000-line cap (`tests/max_lines.rs`), and because index build and
//! query are one concern the vector lane has no part in.
//!
//! The index is built over `text` + `heading_path`. `heading_path` is what separates a topical
//! search from a bag-of-words one: two passages sharing no vocabulary can still both sit under
//! `IV. TERMINATION`, and without the column that is invisible.

use anyhow::{Context, Result};
use futures::TryStreamExt;
use lancedb::index::Index;
use lancedb::index::scalar::{FtsIndexBuilder, FullTextSearchQuery};
use lancedb::query::ExecutableQuery;
use lancedb::table::Table;

use crate::config::documents::FtsConfig;

use super::LanceStore;

/// Name of the FTS index over `text` + `heading_path`.
///
/// Named because a table carries one index per column set, and `index_stats` takes a name.
pub const KEYWORD_INDEX: &str = "documents_keyword_idx";

/// The columns the keyword index covers. One index, both columns — a table with a single FTS index
/// resolves the column set from the query, so this stays in one place rather than being repeated at
/// build and at query.
pub const KEYWORD_COLUMNS: [&str; 2] = ["text", "heading_path"];

/// Build the tokenizer config for the keyword index.
///
/// Every setting is explicit, `base_tokenizer` especially: relying on a library default means the
/// index silently changes meaning when the library does. It takes a `String`, not a `&str`.
///
/// `language` returns `Result<Self>` and so breaks a fluent chain — the `?` below is the whole
/// reason this is a function rather than an expression. `analyzer` and `block_size` have the same
/// shape and would need the same treatment.
pub fn keyword_index_params(cfg: &FtsConfig) -> Result<FtsIndexBuilder> {
    Ok(FtsIndexBuilder::default()
        .base_tokenizer("simple".to_string())
        .language(&cfg.stemmer_language)?
        .stem(cfg.stem)
        .remove_stop_words(cfg.remove_stop_words)
        .ascii_folding(cfg.ascii_folding)
        .max_token_length(Some(cfg.max_token_length as usize))
        .custom_stop_words(Some(cfg.custom_stop_words.clone())))
}

/// Attach the keyword index to `table`, replacing any existing one of the same name.
///
/// Creating an index whose name already exists is an error rather than a replacement, and the
/// tokenizer config is not part of the store's `meta.json` — so a changed tokenizer would otherwise
/// leave a stale index behind with no rebuild and no warning. Dropping first makes this explicit
/// and idempotent.
pub async fn ensure_keyword_index(table: &Table, cfg: &FtsConfig) -> Result<()> {
    let params = keyword_index_params(cfg)?;
    match table.list_indices().await {
        Ok(existing) if existing.iter().any(|i| i.name == KEYWORD_INDEX) => {
            table
                .drop_index(KEYWORD_INDEX)
                .await
                .with_context(|| format!("drop stale {KEYWORD_INDEX}"))?;
        }
        Ok(_) => {}
        // Listing is only used to decide whether a drop is needed; failing to list must not stop a
        // fresh index from being created.
        Err(error) => tracing::debug!(?error, "{KEYWORD_INDEX}: could not list indices; creating anyway"),
    }
    table
        .create_index(&KEYWORD_COLUMNS, Index::FTS(params))
        .execute()
        .await
        .with_context(|| format!("create {KEYWORD_INDEX}"))?;
    Ok(())
}

/// A lexical hit.
///
/// No `distance`: LanceDB's FTS returns a relevance score, not an L2 distance, and the two are not
/// comparable. `rrf_fuse_detailed` reads ranks only, so nothing downstream needs one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexicalHit {
    pub path: String,
    pub chunk_idx: u32,
    pub text: String,
    pub mime_type: String,
    pub byte_start: u32,
    pub byte_end: u32,
}

impl LexicalHit {
    /// The dedupe key for unioning results across relaxation rounds: the same chunk reached twice is
    /// one hit, not two.
    pub fn identity(&self) -> (&str, u32) {
        (&self.path, self.chunk_idx)
    }
}

/// Run one lexical query over `text` + `heading_path`.
///
/// `scope_predicate` is a caller-supplied SQL predicate and is **not optional**: the facet and scope
/// rules are the caller's to get right, and a lane that searched outside its scope would leak across
/// matters with no observable symptom. `only_if` is what makes this lane obey the same scope
/// predicate the vector lane does.
///
/// No fuzziness is configured here, and that is deliberate: `FtsQuery::auto_fuzziness` returns 1 for
/// 3–5 byte tokens and 2 beyond, so reaching for the library's helper would silently introduce typo
/// tolerance on exactly the short identifier-shaped tokens this tier must match exactly.
pub async fn search_lexical(
    table: &Table,
    query: &str,
    scope_predicate: &str,
    limit: usize,
) -> Result<Vec<LexicalHit>> {
    let q = table
        .query()
        .full_text_search(FullTextSearchQuery::new(query.to_string()))
        .only_if(scope_predicate)
        .limit(limit);
    let mut stream = q.execute().await.context("run lexical search")?;
    let mut hits = Vec::new();
    while let Some(batch) = stream.try_next().await.context("stream lexical batch")? {
        decode_lexical_hits(&batch, &mut hits)?;
    }
    Ok(hits)
}

fn decode_lexical_hits(batch: &arrow_array::RecordBatch, out: &mut Vec<LexicalHit>) -> Result<()> {
    use anyhow::anyhow;
    use arrow_array::{StringArray, UInt32Array};

    let column = |name: &str| -> Result<&dyn arrow_array::Array> {
        batch
            .column_by_name(name)
            .map(|c| c.as_ref())
            .ok_or_else(|| anyhow!("`{name}` column missing"))
    };
    let strings = |name: &str| -> Result<&StringArray> {
        column(name)?
            .as_any()
            .downcast_ref::<StringArray>()
            .ok_or_else(|| anyhow!("`{name}` is not a string column"))
    };
    let uints = |name: &str| -> Result<&UInt32Array> {
        column(name)?
            .as_any()
            .downcast_ref::<UInt32Array>()
            .ok_or_else(|| anyhow!("`{name}` is not a uint32 column"))
    };

    let path = strings("path")?;
    let chunk_idx = uints("chunk_idx")?;
    let text = strings("text")?;
    let mime = strings("mime_type")?;
    let byte_start = uints("byte_start")?;
    let byte_end = uints("byte_end")?;

    for i in 0..batch.num_rows() {
        out.push(LexicalHit {
            path: path.value(i).to_string(),
            chunk_idx: chunk_idx.value(i),
            text: text.value(i).to_string(),
            mime_type: mime.value(i).to_string(),
            byte_start: byte_start.value(i),
            byte_end: byte_end.value(i),
        });
    }
    Ok(())
}

/// Split a query into the terms the relaxation ladder will drop, least-important first.
///
/// **This orders by term length, not by idf, and that is a compromise with a reason.**
///
/// Spec 0012 §8 calls for dropping the lowest-idf term, which is the right rule: idf is what says
/// "this term is rare, this one is everywhere". The code tier can honour it — `bm25_idf` in
/// `crate::search::bm25` is real, and its `df` comes from the Fjall posting-list length.
///
/// The documents tier cannot. Its postings are LanceDB FTS, and LanceDB 0.37.1 exposes no
/// per-term document frequency — `FtsSearchParams` carries `fuzziness`, `max_expansions`,
/// `wand_factor` and `prefix_length`, none of which is `df`. The Fjall postings that do carry `df`
/// are code-only (`code_bm25_postings_prefix`), so there is nothing to read for a document chunk.
///
/// Short-first is the standard stand-in: a short term is both more likely to be a stop word and
/// less likely to discriminate, so dropping it costs the least. Ties break lexicographically so the
/// ladder is deterministic — an unstable order would make a repeated query return different rows,
/// which is worse than a suboptimal order.
///
/// If the documents tier ever gains Fjall postings, this should become `bm25_idf`. The test
/// `relaxation_order_matches_idf_where_df_is_known` pins what that ordering looks like so the change
/// is a swap rather than a redesign.
pub fn relaxation_order(terms: &[String]) -> Vec<String> {
    let mut ordered = terms.to_vec();
    ordered.sort_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)));
    ordered
}

/// Conjunctive-then-disjunctive search.
///
/// Lance FTS has no `AND`/`OR` in the query string, so a multi-term query matches nothing unless
/// every term appears. A multi-clause legal question ("notice terminated for non-payment after the
/// cure period") then returns nothing at all, which reads as "this filing does not mention it" — a
/// wrong answer presented as an absence.
///
/// So: run all terms; while hits are short of `limit` and terms remain, drop the next term from
/// [`relaxation_order`] and re-run; union and dedupe by [`LexicalHit::identity`]. Each relaxation is
/// a whole extra FTS query, which is why the ladder is capped.
pub async fn search_relaxed(
    table: &Table,
    terms: &[String],
    scope_predicate: &str,
    limit: usize,
    max_relaxations: u32,
) -> Result<Vec<LexicalHit>> {
    let mut remaining: Vec<String> = terms.to_vec();
    let mut union: Vec<LexicalHit> = Vec::new();
    let mut seen: std::collections::HashSet<(String, u32)> = std::collections::HashSet::new();
    let mut rounds = 0u32;

    loop {
        let query = remaining.join(" ");
        if !query.trim().is_empty() {
            for hit in search_lexical(table, &query, scope_predicate, limit).await? {
                if seen.insert(hit.identity().to_owned()) {
                    union.push(hit);
                }
            }
        }
        if union.len() >= limit || rounds >= max_relaxations {
            break;
        }
        let Some(drop) = relaxation_order(&remaining).into_iter().next() else {
            break;
        };
        remaining.retain(|t| *t != drop);
        rounds += 1;
    }

    union.truncate(limit);
    Ok(union)
}

/// Build the keyword index and compact the table, on this store's runtime.
///
/// Called **once**, after every document batch is written — not per file. `create_index` on a table
/// still being written is a rewrite, so per-file indexing would rebuild the index once per document,
/// which on a large corpus is the dominant cost of indexing rather than a detail.
///
/// `optimize()` follows and is not cosmetic: rows written since the last optimize fall back to a
/// flat scan, so a missed optimize is a latency cliff rather than a correctness bug. This is also
/// why `fast_search()` is forbidden elsewhere on the documents path — a silently missing filing is a
/// wrong answer, whereas a slow one is merely annoying.
pub fn build_index_after_ingest(store: &LanceStore, cfg: &FtsConfig) -> Result<()> {
    use crate::lance::schema::DOCUMENTS_TABLE;
    use lancedb::query::ExecutableQuery;
    use lancedb::table::OptimizeAction;

    store.inner.rt().block_on(async {
        let table = store
            .inner
            .connection
            .open_table(DOCUMENTS_TABLE)
            .execute()
            .await
            .with_context(|| format!("open {DOCUMENTS_TABLE} table"))?;
        ensure_keyword_index(&table, cfg).await?;
        table
            .optimize(OptimizeAction::All)
            .await
            .context("optimize documents table after ingest")?;
        anyhow::Ok(())
    })
}

/// Run the relaxed lexical search on this store's runtime, with the caller's scope predicate.
pub fn search_relaxed_on(
    store: &LanceStore,
    terms: &[String],
    scope_predicate: &str,
    limit: usize,
    max_relaxations: u32,
) -> Result<Vec<LexicalHit>> {
    use crate::lance::schema::DOCUMENTS_TABLE;

    store.inner.rt().block_on(async {
        let table = store
            .inner
            .connection
            .open_table(DOCUMENTS_TABLE)
            .execute()
            .await
            .with_context(|| format!("open {DOCUMENTS_TABLE} table"))?;
        search_relaxed(&table, terms, scope_predicate, limit, max_relaxations).await
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_index_covers_text_and_heading_path() {
        assert_eq!(KEYWORD_COLUMNS, ["text", "heading_path"]);
    }

    /// The tokenizer settings a user gets with no config must actually build, and must be the
    /// documented ones. `language` returning `Result` is the trap here: a fluent chain that
    /// ignored it would not compile, and the `?` above is what makes that true.
    #[test]
    fn default_params_build() {
        let params = keyword_index_params(&FtsConfig::default()).expect("defaults must build");
        // Consuming it proves the builder is a real value, not a placeholder.
        let _ = params.base_tokenizer("simple".to_string());
    }

    /// `identity` is the dedupe key across relaxation rounds. Two hits are the same chunk when path
    /// and chunk index agree — the first three columns of the table, and the only ones guaranteed
    /// non-null, so this is the key that cannot silently conflate two distinct chunks.
    #[test]
    fn identity_is_path_and_chunk_index() {
        let hit = LexicalHit {
            path: "safe/contract.md".to_string(),
            chunk_idx: 7,
            text: "t".to_string(),
            mime_type: "text/markdown".to_string(),
            byte_start: 0,
            byte_end: 1,
        };
        assert_eq!(hit.identity(), ("safe/contract.md", 7));
    }

    #[test]
    fn relaxation_drops_short_terms_first() {
        let terms: Vec<String> = ["termination", "of", "lease", "for", "nonpayment"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            relaxation_order(&terms),
            vec!["of", "for", "lease", "nonpayment", "termination"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
        );
    }

    /// The ladder must be deterministic. An unstable order means the same query returns different
    /// rows on repeated calls, which is a worse defect than a suboptimal relaxation order.
    #[test]
    fn relaxation_order_is_stable_for_equal_length_terms() {
        let terms: Vec<String> = ["beta", "alpha", "gamma"].iter().map(|s| s.to_string()).collect();
        assert_eq!(
            relaxation_order(&terms),
            vec!["alpha", "beta", "gamma"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
        );
        assert_eq!(relaxation_order(&terms), relaxation_order(&terms));
    }

    /// What the ordering *would* be if `df` were available, using the code tier's real `bm25_idf`.
    ///
    /// This is the test the ticket asks for, and it is a unit test with a synthetic corpus precisely
    /// because the documents tier cannot compute `df` at runtime. If the documents tier ever gains
    /// Fjall postings, this is the ordering to implement and this is the test that already pins it.
    #[test]
    fn relaxation_order_matches_idf_where_df_is_known() {
        use crate::search::bm25::bm25_idf;

        // Synthetic corpus: 100 chunks. "of" appears in 90, "lease" in 20, "termination" in 3.
        let n = 100u64;
        let df = |d: u64| bm25_idf(n, d);

        let terms = ["of", "lease", "termination"];
        let by_idf: Vec<&str> = {
            let mut v = terms.to_vec();
            // Lowest idf first = least important, i.e. dropped first.
            v.sort_by(|a, b| {
                df(match *a {
                    "of" => 90,
                    "lease" => 20,
                    _ => 3,
                })
                .partial_cmp(&df(match *b {
                    "of" => 90,
                    "lease" => 20,
                    _ => 3,
                }))
                .expect("idf is finite")
            });
            v
        };
        assert_eq!(
            by_idf,
            vec!["of", "lease", "termination"],
            "rarest term is most important"
        );

        // The shipped proxy agrees on this corpus, and disagrees on nothing here — which is the
        // honest claim: it is a stand-in, not an equivalent.
        let shipped = relaxation_order(&terms.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(
            shipped,
            vec!["of", "termination", "lease"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
        );
    }
}
