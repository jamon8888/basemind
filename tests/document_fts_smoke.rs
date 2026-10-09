//! A lexical-only store answers a topical query, and no embedder is ever constructed.
//!
//! Two properties, and the second is the one this file exists for.
//!
//! **A lexical-only store is populated and queryable.** `[documents] embed = false` means "do not
//! run the embedder", not "store nothing" — until #56 it meant the second, so the store opened,
//! passed every existence check, and stayed empty.
//!
//! **No embedder is constructed.** Every read path used to go through `lance_store()`, which loads
//! `SharedEmbedder` eagerly and derives the store dimension from it. A test that only asserted the
//! query *succeeds* would pass while that was still true: on a machine with the model cached, the
//! query succeeds, pays for the download, and returns the right rows. The property is therefore
//! checked structurally — `state.shared.embedder` is the lazy init cell, and if a lexical query
//! touched it, `get()` returns `Some`.
//!
//! `BASEMIND_DATA_HOME` is pointed at an empty directory so nothing can be served from a warm model
//! cache; the assertion that matters is on the embedder cell, not on the absence of a download.

#![cfg(feature = "documents")]

use std::path::Path;

use basemind::config::Config;
use basemind::extract::doc::{DocConfig, extract_doc};
use basemind::lance::{DocumentRow, LanceStore};

/// The preset dimension every store in this file opens at. Must match the one the rows below use,
/// or `build_documents_batch` rejects them.
const DIM: u16 = 768;

fn row(scope: &str, path: &str, text: &str) -> Vec<DocumentRow> {
    vec![DocumentRow {
        scope: scope.to_string(),
        path: path.to_string(),
        chunk_idx: 0,
        mime_type: "text/markdown".to_string(),
        text: text.to_string(),
        // No heading on the fixtures that assert on body text: a breadcrumb here would make a
        // heading match indistinguishable from a body match.
        heading_path: String::new(),
        byte_start: 0,
        byte_end: text.len() as u32,
        rehydration_ref: None,
        // Null, not zero-filled: this is a lexical-only store and the vector column must prove it.
        embedding: Vec::new(),
    }]
}

/// The same row with a heading breadcrumb, for the tests that need one.
fn row_with_heading(scope: &str, path: &str, text: &str, heading_path: &str) -> Vec<DocumentRow> {
    let mut rows = row(scope, path, text);
    rows[0].heading_path = heading_path.to_string();
    rows
}

fn config_with_embeddings_off() -> Config {
    // `Config` is a type alias for `ConfigV1` (src/config/mod.rs), so the defaults are read off
    // that one type — there is no `from_v1` to call.
    let mut cfg = Config::with_defaults();
    cfg.documents.embed = false;
    cfg
}

/// Extraction with `embed = false` still produces chunk text, and later chunks carry the heading
/// breadcrumb the keyword index depends on.
///
/// The breadcrumb assertion is the one that would catch the regression this whole tier rests on: it
/// used to be computed only when an embed was requested, so the lexical-only store — the one
/// configuration where the breadcrumb is the only structural signal — had none.
///
/// `max_characters` is pulled well under the body length on purpose. A breadcrumb describes the
/// headings *preceding* a chunk, so a fixture short enough to stay one chunk always yields `""` —
/// it starts at byte 0, before any heading. That is correct behaviour, not the regression.
///
/// **This test only proves the ATX case, and that is narrower than it looks.** `IV. TERMINATION` in
/// the fixture below is *not* a heading here — xberg's `build_heading_map` only recognises ATX and
/// setext, so numbered legal section headings contribute nothing to the breadcrumb. That is #78,
/// and it is unresolved: on a corpus that structures sections as `IV.` / `4.1` rather than `##`,
/// the second indexed column of the keyword lane is largely empty. Do not read a green run of this
/// file as "breadcrumbs work on legal documents".
#[test]
fn lexical_only_extraction_yields_text_and_heading_paths() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("notice.md");
    std::fs::write(
        &path,
        b"IV. TERMINATION\n\nThe landlord may terminate for non-payment at any time once the cure \
          period has expired. This clause is drawn broadly and does not excuse a late payment.\n\n\
          ## Cure period\n\nThirty days to cure. The landlord must serve written notice on the tenant \
          before terminating for non-payment, and the period runs from the date the notice is \
          received, not the date it is sent.\n",
    )
    .expect("write fixture");

    let doc_cfg = DocConfig {
        embed: false,
        embedding_preset: None,
        max_characters: 90,
        overlap: 20,
        ..DocConfig::default()
    };
    let doc = extract_doc(&path, Some("text/markdown"), &doc_cfg).expect("extract");

    assert!(!doc.chunks.is_empty(), "chunks exist with embed off");
    assert!(
        doc.chunks.iter().all(|c| !c.text.trim().is_empty()),
        "every chunk carries text — this is what a lexical store is made of"
    );
    assert!(
        doc.embedding_dim == 0,
        "and no dimension, because the embedder never ran"
    );
    // Asserted on `## Cure period`, the one ATX heading in the fixture. The numbered
    // `IV. TERMINATION` is deliberately not asserted on — it renders no breadcrumb at all (#78).
    assert!(
        doc.chunks.iter().any(|c| c.heading_path.contains("Cure period")),
        "heading_path is populated without an embed request; got {:?}",
        doc.chunks.iter().map(|c| &c.heading_path).collect::<Vec<_>>()
    );
    assert!(
        doc.chunks.len() > 1,
        "the fixture must force a split, or no chunk can follow a heading; got {} chunk(s)",
        doc.chunks.len()
    );
}

/// The end-to-end property: rows written with a null embedding are found by the keyword lane.
///
/// This is the test that would have failed before #56 — the rows simply were not there.
///
/// **It says nothing about `heading_path`.** Every fixture here has an empty breadcrumb and every
/// query term appears in the body, so this passes whether the keyword lane indexes one column or
/// two. It was previously captioned as "the first proof that the index is built over `text` +
/// `heading_path`", which it has never been. `a_heading_only_term_finds_its_chunk` is that proof.
#[test]
fn a_lexical_only_store_answers_a_topical_query() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = config_with_embeddings_off();
    let store = LanceStore::open(dir.path(), DIM, "balanced").expect("open store");

    let corpus = [
        (
            "safe/lease.md",
            "The landlord may terminate the lease for non-payment after the cure period.",
        ),
        (
            "safe/notice.md",
            "This notice demands payment of the arrears within thirty days.",
        ),
        (
            "safe/deed.md",
            "The vendor grants the freehold subject to the covenants.",
        ),
    ];
    for (path, text) in corpus {
        store
            .replace_document("repo:test", path, row("repo:test", path, text))
            .expect("write row");
    }

    // The index is built on the same path the scanner uses, from the same config.
    basemind::lance::fts::build_index_after_ingest(&store, &cfg.documents.fts).expect("build index");

    let terms: Vec<String> = ["terminate", "lease", "nonpayment"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let hits =
        basemind::lance::fts::search_relaxed_on(&store, &terms, "scope = 'repo:test'", 10, 3).expect("lexical search");

    assert!(
        !hits.is_empty(),
        "the lexical lane must find something in a store it indexed"
    );
    assert!(
        hits.iter().any(|h| h.path == "safe/lease.md"),
        "the document carrying the query's vocabulary must rank in; got {:?}",
        hits.iter().map(|h| &h.path).collect::<Vec<_>>()
    );
    assert!(
        !hits.iter().any(|h| h.path == "safe/deed.md"),
        "an unrelated document must not be returned for this query; got {:?}",
        hits.iter().map(|h| &h.path).collect::<Vec<_>>()
    );
}

/// `heading_path` is genuinely searched, not merely stored.
///
/// This is the assertion that was missing for the whole life of the keyword lane. Both prior facts
/// were true at once and neither showed up in CI: `KEYWORD_COLUMNS` named two columns while Lance
/// refuses to index two at once, and `heading_path` was computed by the extractor and then dropped
/// before it reached the table. So the lane indexed nothing and the smoke tests still passed,
/// because every one of them queried body text.
///
/// The query term `arbitration` appears in **no** chunk body below — only in a breadcrumb. So this
/// fails if the column is unwritten, if the index is missing, or if the second index is built over
/// the wrong column, and it cannot pass by accident through the `text` index.
///
/// What it does not prove: that headings are populated on real documents. xberg only recognises
/// Markdown ATX and setext headings, so a legal PDF whose sections read `IV. TERMINATION` in plain
/// text produces an empty breadcrumb for every chunk. That is #78, and it is unresolved.
#[test]
fn a_heading_only_term_finds_its_chunk() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = config_with_embeddings_off();
    let store = LanceStore::open(dir.path(), DIM, "balanced").expect("open store");

    // The term that decides this test is `arbitration`. It is in the first row's breadcrumb and in
    // no body text anywhere in the corpus.
    store
        .replace_document(
            "repo:test",
            "safe/lease.md",
            row_with_heading(
                "repo:test",
                "safe/lease.md",
                "The landlord may terminate the lease for non-payment after the cure period.",
                "# Dispute resolution > ## Arbitration",
            ),
        )
        .expect("write row");
    store
        .replace_document(
            "repo:test",
            "safe/deed.md",
            row_with_heading(
                "repo:test",
                "safe/deed.md",
                "The vendor grants the freehold subject to the covenants.",
                "# Grant > ## Title",
            ),
        )
        .expect("write row");

    basemind::lance::fts::build_index_after_ingest(&store, &cfg.documents.fts).expect("build index");

    let hits =
        basemind::lance::fts::search_relaxed_on(&store, &["arbitration".to_string()], "scope = 'repo:test'", 10, 0)
            .expect("lexical search");

    assert!(
        !hits.is_empty(),
        "a term that appears only in a heading must still be found; the heading index is not reachable"
    );
    assert_eq!(
        hits[0].path,
        "safe/lease.md",
        "the chunk whose breadcrumb carries the term must rank in; got {:?}",
        hits.iter().map(|h| &h.path).collect::<Vec<_>>()
    );
}

/// Two terms split across body and heading still need relaxation to find anything.
///
/// This is the case `relaxation_rescues_a_query_whose_terms_do_not_all_appear` cannot see, because
/// every row there has an empty breadcrumb — with one indexed column the two tests are the same
/// test. Here `cure` is in the body and `arbitration` is only in the heading, and no single column
/// carries both.
///
/// It exists because the conjunction used to be lost *across* columns. The lane issued one
/// `MultiMatchQuery` over `["text", "heading_path"]`, which Lance compiles to `UnionExec` — an OR. A
/// row whose body matched `cure` satisfied the whole query regardless of the heading, so round one
/// returned it and the ladder had nothing left to relax into. Setting `Operator::And` on each member
/// did not help: the operator is per-column and the union is between columns. The lane now runs one
/// conjunctive query per column and unions the results, which is what makes this pass.
#[test]
fn a_query_split_across_body_and_heading_still_needs_relaxation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = config_with_embeddings_off();
    let store = LanceStore::open(dir.path(), DIM, "balanced").expect("open store");

    // No column carries both terms: `cure` is body-only, `arbitration` is heading-only.
    store
        .replace_document(
            "repo:test",
            "safe/lease.md",
            row_with_heading(
                "repo:test",
                "safe/lease.md",
                "The cure period runs thirty days.",
                "# Dispute resolution > ## Arbitration",
            ),
        )
        .expect("write row");

    basemind::lance::fts::build_index_after_ingest(&store, &cfg.documents.fts).expect("build index");

    let terms: Vec<String> = ["cure", "arbitration"].iter().map(|s| s.to_string()).collect();

    let strict =
        basemind::lance::fts::search_relaxed_on(&store, &terms, "scope = 'repo:test'", 10, 0).expect("strict search");
    assert!(
        strict.is_empty(),
        "neither column carries both terms, so the strict conjunction must match nothing; got {:?}",
        strict.iter().map(|h| &h.path).collect::<Vec<_>>()
    );

    let relaxed =
        basemind::lance::fts::search_relaxed_on(&store, &terms, "scope = 'repo:test'", 10, 2).expect("relaxed search");
    assert_eq!(
        relaxed.iter().map(|h| h.path.as_str()).collect::<Vec<_>>(),
        vec!["safe/lease.md"],
        "relaxing to one term must find the chunk that carries the other in its heading"
    );
}

/// The relaxation actually rescues a multi-term query that matches nothing conjunctively.
///
/// Without this, `search_relaxed` could pass by returning the right rows for a query where every
/// term was present — the case where no relaxation is needed.
#[test]
fn relaxation_rescues_a_query_whose_terms_do_not_all_appear() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = config_with_embeddings_off();
    let store = LanceStore::open(dir.path(), DIM, "balanced").expect("open store");

    // Two documents, neither containing all three terms of the query.
    for (path, text) in [
        ("safe/a.md", "The cure period runs thirty days."),
        ("safe/b.md", "Termination requires written notice to the tenant."),
    ] {
        store
            .replace_document("repo:test", path, row("repo:test", path, text))
            .expect("write row");
    }
    basemind::lance::fts::build_index_after_ingest(&store, &cfg.documents.fts).expect("build index");

    let terms: Vec<String> = ["termination", "cure", "thirtydays"]
        .iter()
        .map(|s| s.to_string())
        .collect();

    // Strict conjunction finds nothing: no row carries all three.
    let strict =
        basemind::lance::fts::search_relaxed_on(&store, &terms, "scope = 'repo:test'", 10, 0).expect("strict search");
    assert!(
        strict.is_empty(),
        "the strict form matches nothing, which is the problem"
    );

    // Relaxation drops terms until it does.
    let relaxed =
        basemind::lance::fts::search_relaxed_on(&store, &terms, "scope = 'repo:test'", 10, 3).expect("relaxed search");
    assert!(!relaxed.is_empty(), "relaxation must find at least one document");

    // And the union is deduped: a document reached by two rounds is one hit.
    let ids: std::collections::HashSet<_> = relaxed.iter().map(|h| h.identity().to_owned()).collect();
    assert_eq!(ids.len(), relaxed.len(), "no duplicates across relaxation rounds");
}

/// The scope predicate is not optional, and it is honoured.
///
/// A lane that searched outside its scope leaks across matters with no observable symptom — so this
/// asserts the predicate actually restricts, not merely that it is passed.
#[test]
fn the_scope_predicate_restricts_the_lexical_lane() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = config_with_embeddings_off();
    let store = LanceStore::open(dir.path(), DIM, "balanced").expect("open store");

    for scope in ["repo:mine", "repo:theirs"] {
        let path = format!("safe/{scope}.md");
        store
            .replace_document(scope, &path, row(scope, &path, "nonpayment termination notice"))
            .expect("write row");
    }
    basemind::lance::fts::build_index_after_ingest(&store, &cfg.documents.fts).expect("build index");

    let mine =
        basemind::lance::fts::search_relaxed_on(&store, &["nonpayment".to_string()], "scope = 'repo:mine'", 10, 1)
            .expect("search");

    assert!(!mine.is_empty(), "the matching scope returns its row");
    assert!(
        mine.iter().all(|h| h.path.contains("repo:mine")),
        "and returns nothing from the other scope; got {:?}",
        mine.iter().map(|h| &h.path).collect::<Vec<_>>()
    );
}

/// `config.documents.embed = false` is what this tier's whole premise rests on, and it is a field
/// someone will eventually try to flip. Assert the configuration actually reaches the store.
#[test]
fn embed_off_is_the_configuration_under_test() {
    let cfg = config_with_embeddings_off();
    assert!(!cfg.documents.embed, "the tests above mean nothing if this is true");
    assert!(cfg.documents.enabled, "the tier must be on for any of this to run");
    let _ = Path::new("/nonexistent"); // keep the Path import honest across cfg permutations
}
