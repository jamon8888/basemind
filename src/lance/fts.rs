//! Lexical (full-text) retrieval for the documents tier.
//!
//! Split out of `mod.rs` by the 1000-line cap (`tests/max_lines.rs`), and because index build and
//! query are one concern the vector lane has no part in.
//!
//! The keyword lane covers `text` + `heading_path`. `heading_path` is what separates a topical
//! search from a bag-of-words one: two passages sharing no vocabulary can still both sit under the
//! same heading, and without the column that is invisible.
//!
//! **One index per column, not one index over both.** Lance refuses to build an inverted index over
//! more than one column — `lance-10.0.0/src/index/create.rs:144` and
//! `lancedb-0.37.1/src/table/create_index.rs:117` both reject `columns.len() != 1` outright, and
//! neither is behind a feature flag or a version guard. So this is two `create_index` calls.
//!
//! **What the query side needs: name the columns, and set the operator.** Lance does resolve a
//! columnless query by discovering every FTS-indexed column and building a `MultiMatchQuery` — but
//! that resolution throws away everything except the term string: `MultiMatchQuery::try_new` builds
//! a **fresh** `MatchQuery` per column (`lance-index-10.0.0/src/scalar/inverted/query.rs:848-853`), so
//! an operator set on the incoming query never reaches the ones that run. The lane therefore builds
//! its own query. See [`keyword_query`].
//!
//! **On what a `MultiMatchQuery` over two columns actually does**, because it has been got wrong
//! here once and the wrong version is not obviously wrong to read: `MultiMatchQuery` compiles to
//! `UnionExec` (`lance-10.0.0/src/dataset/scanner.rs:3437-3451`), and that union is across
//! **columns**, not across **terms**. Each member is planned with its own operator, so a row is
//! returned when *some* column carries every term. Measured on `lancedb 0.37.1` / `lance 10.0.0`
//! with `And` on both members, over a corpus where `lease.md` holds `terminate` in `text` and
//! `Arbitration` in `heading_path` and `deed.md` holds `covenants` in `text`:
//!
//! | query | result |
//! |---|---|
//! | `terminate arbitration` (one term per column) | `[]` |
//! | `terminate lease` (both terms in `text`) | `[lease.md]` |
//! | `terminate covenants` (a body term from each of two rows) | `[]` |
//!
//! The third row is the one that settles it: a plain union would have returned both rows. So the
//! conjunction holds across columns, and [`search_relaxed`] has something real to relax.
//!
//! **What the union still costs, stated plainly:** it takes a *maximum*, not a sum. A chunk that
//! matches weakly on its heading and strongly on its body scores the same as one that matches only
//! on its heading. A true composite index would have added the two contributions. `MultiMatchQuery`
//! exposes `try_with_boosts` if real corpora show this needs weighting; nothing is weighted today
//! because nothing has been measured yet.

use anyhow::{Context, Result};
use futures::TryStreamExt;
use lancedb::index::Index;
use lancedb::index::scalar::{FtsIndexBuilder, FtsQuery, FullTextSearchQuery, MatchQuery, MultiMatchQuery, Operator};
// `full_text_search` is on the `QueryBase` trait, not inherent on `Query` — without this import
// the method does not resolve, and the error names `struct lancedb::query::Query`, which points at
// the type rather than at the missing trait in scope.
use lancedb::query::{ExecutableQuery, QueryBase};
use lancedb::table::Table;

use crate::config::FtsConfig;

use super::LanceStore;

/// The keyword indexes on `documents`, as `(index name, indexed column)`.
///
/// The names are set explicitly on every `create_index` call. Lance derives a default name from the
/// column when none is given — `text_idx` — and the previous code compared `list_indices` against a
/// name it never asked for, so its "drop the stale index" branch could never fire and a changed
/// tokenizer silently left the old index in place. Naming them here is what makes that branch real.
///
/// Order matters only for readability; discovery at query time is schema order, not this order.
pub const KEYWORD_INDEXES: [(&str, &str); 2] = [
    ("documents_text_idx", "text"),
    ("documents_heading_idx", "heading_path"),
];

/// The columns the keyword lane covers.
///
/// Every one of them carries an FTS index, and that is the contract: `search_lexical` names no
/// column, so Lance searches exactly the columns it finds indexed. A column listed here with no
/// index behind it would be silently absent from every search rather than erroring.
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

/// Attach the keyword indexes to `table`, replacing any existing ones of the same name.
///
/// Creating an index whose name already exists is an error rather than a replacement, and the
/// tokenizer config is not part of the store's `meta.json` — so a changed tokenizer would otherwise
/// leave a stale index behind with no rebuild and no warning. Dropping first makes this explicit
/// and idempotent.
///
/// The index list is read once, before the loop, and the drops are not read back between creates:
/// each index is named for the column it covers, so no create can collide with another entry's
/// drop.
pub async fn ensure_keyword_index(table: &Table, cfg: &FtsConfig) -> Result<()> {
    let params = keyword_index_params(cfg)?;
    // Listing is only used to decide whether a drop is needed; failing to list must not stop a
    // fresh index from being created.
    let existing = match table.list_indices().await {
        Ok(indices) => indices,
        Err(error) => {
            tracing::debug!(?error, "keyword indexes: could not list indices; creating anyway");
            Vec::new()
        }
    };
    for (name, column) in KEYWORD_INDEXES {
        if existing.iter().any(|i| i.name == name) {
            table
                .drop_index(name)
                .await
                .with_context(|| format!("drop stale {name}"))?;
        }
        table
            .create_index(&[column], Index::FTS(params.clone()))
            .name(name.to_string())
            .execute()
            .await
            .with_context(|| format!("create {name} on {column}"))?;
    }
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

/// Build the conjunctive keyword-lane query for `terms` over every indexed column.
///
/// **This cannot be left to Lance's column auto-discovery.** `fill_fts_query_column`
/// (`lance-index-10.0.0/src/scalar/inverted/query.rs:848`) handles a columnless query by
/// discovering every FTS-indexed column and calling `MultiMatchQuery::try_new`, which builds a
/// **fresh** `MatchQuery` per column from `terms` alone. Anything set on the incoming query — the
/// operator, the fuzziness — is discarded on that path. Since the operator is the whole point, the
/// query is built here.
///
/// Two settings, both deliberate:
///
/// - **`Operator::And`.** `MatchQuery::new` defaults to `Or` (`query.rs:333`). Left at the default, a
///   question like *"notice terminated for non-payment after the cure period"* matches any document
///   containing any one of those words, and the conjunction [`search_relaxed`] exists to relax has
///   nothing to relax. `And` is what makes a multi-term query answer about its subject rather than
///   about its most distinctive word.
/// - **`fuzziness: Some(0)`, set explicitly.** The exactness this tier needs for identifier-shaped
///   tokens comes from `MatchQuery::new` defaulting to `Some(0)`, not from anything this code does.
///   A comment here once claimed the *absence* of a setting was the reason, which was backwards, and
///   pointed the reader at `MatchQuery::auto_fuzziness` (1 for 3–5 byte tokens, 2 beyond) as the
///   thing to avoid — as though the lane were at risk of opting in. Stating `Some(0)` makes the
///   guarantee ours rather than a side effect of a default that can change under us.
///
/// **One query, not one per column.** Querying each column separately and unioning in Rust produces
/// the same rows — `MultiMatchQuery` is already a union of per-column plans — but costs a second FTS
/// query per relaxation round, and needs `_score` plumbed into [`LexicalHit`] to do the max-by-score
/// merge that the planner already performs. It was tried, on the belief that `UnionExec` made the
/// whole query disjunctive; the measurements in the module docs show it does not.
pub fn keyword_query(terms: &str) -> Result<FullTextSearchQuery> {
    let match_queries = KEYWORD_COLUMNS
        .iter()
        .map(|column| {
            MatchQuery::new(terms.to_string())
                .with_column(Some((*column).to_string()))
                .with_operator(Operator::And)
                .with_fuzziness(Some(0))
        })
        .collect();
    Ok(FullTextSearchQuery::new_query(FtsQuery::MultiMatch(MultiMatchQuery {
        match_queries,
    })))
}

/// Run one lexical query over `text` + `heading_path`.
///
/// `scope_predicate` is a caller-supplied SQL predicate and is **not optional**: the facet and scope
/// rules are the caller's to get right, and a lane that searched outside its scope would leak across
/// matters with no observable symptom. `only_if` is what makes this lane obey the same scope
/// predicate the vector lane does.
pub async fn search_lexical(
    table: &Table,
    query: &str,
    scope_predicate: &str,
    limit: usize,
) -> Result<Vec<LexicalHit>> {
    let q = table
        .query()
        .full_text_search(keyword_query(query)?)
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
/// [`keyword_query`] asks for `Operator::And`, so the first round is a true conjunction and a
/// multi-clause legal question ("notice terminated for non-payment after the cure period") matches
/// only documents that really carry all of it. Too strict on its own: if one term is absent from a
/// document, that document is excluded even when it plainly answers the question, and to a user
/// that reads as "this filing does not mention it" — a wrong answer presented as an absence.
///
/// So: run all terms; while hits are short of `limit` and terms remain, drop the next term from
/// [`relaxation_order`] and re-run; union and dedupe by [`LexicalHit::identity`]. Each relaxation is
/// a whole extra FTS query, which is why the ladder is capped.
///
/// **This ladder only means anything because the conjunction holds across columns.** At Lance's
/// default `Or` the first round is already disjunctive and every later round can only add rows the
/// first one already covered, so the function degenerates into a single OR query with extra steps —
/// which is what it did until the operator was set. `relaxation_rescues_a_query_whose_terms_do_not_all_appear`
/// and `a_query_split_across_body_and_heading_still_needs_relaxation` are what hold this honest, and
/// why the query shape is asserted as well as the operator: asserting `And` on the query object
/// alone once passed while the lane was still wrong for two columns.
pub async fn search_relaxed(
    table: &Table,
    terms: &[String],
    scope_predicate: &str,
    limit: usize,
    max_relaxations: u32,
) -> Result<Vec<LexicalHit>> {
    let mut remaining: Vec<String> = terms.to_vec();
    let mut union: Vec<LexicalHit> = Vec::new();
    // `(&str, u32).to_owned()` is `(&str, u32)`, not `(String, u32)`: `ToOwned` on `&str` has
    // `Owned = str`. The path is cloned explicitly so the key owns its string and the `hits` vector
    // can move on.
    let mut seen: std::collections::HashSet<(String, u32)> = std::collections::HashSet::new();
    let mut rounds = 0u32;

    loop {
        let query = remaining.join(" ");
        if !query.trim().is_empty() {
            for hit in search_lexical(table, &query, scope_predicate, limit).await? {
                if seen.insert((hit.path.clone(), hit.chunk_idx)) {
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

    /// Every column the lane claims to cover must have an index built for it, and every index must
    /// name the column it covers.
    ///
    /// The two halves fail in opposite directions and both fail silently. An extra `KEYWORD_COLUMNS`
    /// entry with no index behind it is simply absent from every search; an index named for a
    /// different column is searched instead of the one intended. `search_lexical` names no column,
    /// so Lance works from what it finds on disk — this is the only place the two lists are checked
    /// against each other.
    #[test]
    fn every_indexed_column_has_an_index_named_for_it() {
        let indexed: Vec<&str> = KEYWORD_INDEXES.iter().map(|(_, column)| *column).collect();
        assert_eq!(
            indexed, KEYWORD_COLUMNS,
            "KEYWORD_COLUMNS and KEYWORD_INDEXES must name the same columns in the same order"
        );
        let names: Vec<&str> = KEYWORD_INDEXES.iter().map(|(name, _)| *name).collect();
        let mut unique = names.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), names.len(), "index names must be distinct: {names:?}");
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
    /// Every column the lane indexes must be a conjunct of its own query, matched exactly.
    ///
    /// Both settings are invisible in the result shape, so nothing else would catch a regression:
    /// a lane that silently fell back to `Or` still returns hits, still returns the right hits for a
    /// single-term query, and still passes every smoke test whose terms all appear in one row. The
    /// difference only shows on a multi-term query, which is why it is asserted on the query object.
    ///
    /// The shape is a `MultiMatch` with one `And` member per indexed column, which is a conjunction
    /// *within* a column and a union *between* columns. A previous version of this lane queried each
    /// column separately in Rust on the belief that `MultiMatch`'s `UnionExec` made the whole query
    /// disjunctive; the measurements in the module docs show that belief was wrong. This test
    /// pins the shape so the question does not get re-litigated from the source alone, and
    /// `a_query_split_across_body_and_heading_still_needs_relaxation` pins the behaviour, because
    /// the shape assertion alone once passed while the lane was wrong for two columns.
    #[test]
    fn the_keyword_query_is_conjunctive_and_exact_on_every_column() {
        let query = keyword_query("terminate lease").expect("query builds");
        assert_eq!(
            query.columns(),
            KEYWORD_COLUMNS
                .iter()
                .map(|c| (*c).to_string())
                .collect::<std::collections::HashSet<_>>(),
            "the query must name exactly the indexed columns"
        );
        let FtsQuery::MultiMatch(multi) = query.query else {
            panic!("one MatchQuery per indexed column, so this must be a MultiMatch");
        };
        assert_eq!(
            multi.match_queries.len(),
            KEYWORD_COLUMNS.len(),
            "every indexed column needs its own conjunct, or it is never searched"
        );
        for match_query in &multi.match_queries {
            let column = match_query.column.as_deref().unwrap_or("<none>");
            assert_eq!(
                match_query.operator,
                Operator::And,
                "column {column:?} would otherwise match any one term, and the ladder would have \
                 nothing to relax"
            );
            assert_eq!(
                match_query.fuzziness,
                Some(0),
                "column {column:?} must match exactly: a 3-byte token gets fuzziness 1 by default, \
                 which would let `368` reach `362`"
            );
        }
    }

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
    ///
    /// "Equal length" in the name does not hold: `beta` is four bytes, `alpha` and `gamma` are
    /// five. Length therefore places `beta` first and lexicographic order breaks the tie between
    /// the other two. The fixture is kept because the tie-break is what this test is for.
    #[test]
    fn relaxation_order_is_stable_for_equal_length_terms() {
        let terms: Vec<String> = ["beta", "alpha", "gamma"].iter().map(|s| s.to_string()).collect();
        assert_eq!(
            relaxation_order(&terms),
            vec!["beta", "alpha", "gamma"]
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

        // The shipped proxy agrees with IDF on this corpus: length and rarity point the same way for
        // all three terms, so the proxy is a stand-in that happens to hold here, not an
        // equivalent. That it would not hold in general is why the ladder is a relaxation and
        // not a substitute for ranking.
        let shipped = relaxation_order(&terms.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(
            shipped,
            vec!["of", "lease", "termination"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
        );
    }
}
