//! The document flush must delete under the same scope it writes under.
//!
//! `flush_document_batches` used to stamp rows with the batch's per-path scope
//! (`batch.doc_scope`, i.e. `path:<extra_root>` for an external-root document) while issuing the
//! delete against the scan-wide scope. `replace_document` is delete-then-insert, so a delete that
//! targets a scope the rows were never written under deletes nothing and every re-extraction adds
//! another copy of the document.
//!
//! The defect is invisible without a row count: both writes succeed, search returns correct-looking
//! results, and the store simply grows. That is why [`LanceStore::count_documents`] exists.
//!
//! No embedding model is loaded here. `replace_document` takes rows, not text, so the vectors are
//! supplied directly — the scanner and its embedder are not what is under test here, the scope
//! arithmetic is.

#![cfg(feature = "documents")]

use basemind::lance::{DocumentRow, LanceStore};

/// A one-chunk document row at the store's dimension.
fn row(scope: &str, path: &str, chunk_idx: u32, text: &str, dim: usize) -> Vec<DocumentRow> {
    vec![DocumentRow {
        scope: scope.to_string(),
        path: path.to_string(),
        chunk_idx,
        mime_type: "text/markdown".to_string(),
        text: text.to_string(),
        heading_path: String::new(),
        byte_start: 0,
        byte_end: text.len() as u32,
        rehydration_ref: None,
        embedding: vec![0.0_f32; dim],
    }]
}

/// Rows written under a per-root scope and then replaced under the **scan-wide** scope: the
/// delete misses, and the old row survives next to the new one.
///
/// This is the defect, stated as an assertion rather than described in a comment. If someone
/// reintroduces the mismatch, this test goes red instead of the store quietly doubling.
#[test]
fn a_delete_under_the_wrong_scope_leaves_the_old_rows_behind() {
    const DIM: u16 = 8;
    const DOC_SCOPE: &str = "path:/extra/root";
    const SCAN_SCOPE: &str = "repo:origin";
    const PATH: &str = "/extra/root/contract.md";

    let dir = tempfile::tempdir().expect("tempdir");
    let store = LanceStore::open(dir.path(), DIM, "balanced").expect("open store");

    // First extraction.
    store
        .replace_document(DOC_SCOPE, PATH, row(DOC_SCOPE, PATH, 0, "version one", DIM as usize))
        .expect("first write");
    assert_eq!(
        store.count_documents(None).expect("count"),
        1,
        "one row after the first extraction"
    );

    // Re-extraction, with the delete aimed at the scan-wide scope instead of the write's scope.
    store
        .replace_document(SCAN_SCOPE, PATH, row(DOC_SCOPE, PATH, 0, "version two", DIM as usize))
        .expect("second write");

    let total = store.count_documents(None).expect("count");
    let stale = store
        .count_documents(Some(&format!("scope = '{SCAN_SCOPE}'")))
        .expect("count by scan scope");
    // Nothing lands under the delete's scope: `replace_document` writes rows carrying their *own*
    // `scope` field, and the row below is built with `DOC_SCOPE`. The delete aimed at `SCAN_SCOPE`
    // matched no row, which is the whole point — see the total assertion.
    assert_eq!(
        stale, 0,
        "the mismatched delete wrote nothing under {SCAN_SCOPE}, so the leaked row is still \
         readable under its own scope {DOC_SCOPE}"
    );
    assert_eq!(
        total, 2,
        "a delete under a scope the rows were never written under leaves the old row in place — \
         this is the leak, and it is what the fix removes"
    );
}

/// The fix: replace under the same scope twice, and the second write *replaces* rather than
/// accumulates. Repeated re-extraction leaves the count flat.
#[test]
fn replacing_under_one_scope_does_not_accumulate_rows() {
    const DIM: u16 = 8;
    const DOC_SCOPE: &str = "path:/extra/root";
    const PATH: &str = "/extra/root/contract.md";

    let dir = tempfile::tempdir().expect("tempdir");
    let store = LanceStore::open(dir.path(), DIM, "balanced").expect("open store");

    for round in 0..4 {
        store
            .replace_document(
                DOC_SCOPE,
                PATH,
                row(DOC_SCOPE, PATH, 0, &format!("version {round}"), DIM as usize),
            )
            .expect("write");
        assert_eq!(
            store.count_documents(None).expect("count"),
            1,
            "after re-extraction {round}: a replace under a matching scope must not accumulate"
        );
    }
}

/// In-repo paths are unaffected: `doc_scope_for` returns the scan-wide scope for them, so the
/// write and the delete were already identical and stay identical.
#[test]
fn a_repo_relative_path_under_the_repo_scope_is_unaffected() {
    const DIM: u16 = 8;
    const REPO_SCOPE: &str = "repo:origin";
    const PATH: &str = "docs/contract.md";

    let dir = tempfile::tempdir().expect("tempdir");
    let store = LanceStore::open(dir.path(), DIM, "balanced").expect("open store");

    for round in 0..3 {
        store
            .replace_document(
                REPO_SCOPE,
                PATH,
                row(REPO_SCOPE, PATH, 0, &format!("version {round}"), DIM as usize),
            )
            .expect("write");
        assert_eq!(store.count_documents(None).expect("count"), 1, "round {round}");
    }
}

/// A document whose scope is the repo scope stays *findable* after the fix. This is the case the
/// earlier draft of the ticket got backwards: writing every external-root document under the
/// scan-wide scope would have made them reachable by default search, but would also have thrown
/// away the per-root partitioning `doc_scope_for` exists to provide. The fix keeps the
/// partitioning *and* makes the delete match the write, so nothing here changes for repo paths and
/// external-root rows remain reachable to a caller that asks for their scope.
#[test]
fn external_scope_rows_stay_reachable_to_a_caller_that_names_the_scope() {
    const DIM: u16 = 8;
    const DOC_SCOPE: &str = "path:/extra/root";
    const PATH: &str = "/extra/root/contract.md";

    let dir = tempfile::tempdir().expect("tempdir");
    let store = LanceStore::open(dir.path(), DIM, "balanced").expect("open store");
    store
        .replace_document(DOC_SCOPE, PATH, row(DOC_SCOPE, PATH, 0, "text", DIM as usize))
        .expect("write");

    assert_eq!(
        store
            .count_documents(Some(&format!("scope = '{DOC_SCOPE}'")))
            .expect("count by doc scope"),
        1,
        "a caller naming `path:/extra/root` finds its rows; `resolve_doc_scope` honours the \
         requested value verbatim (src/mcp/memory.rs), so the partitioning does not make them \
         unreachable — only un-defaulted"
    );
    assert_eq!(
        store
            .count_documents(Some("scope = 'repo:origin'"))
            .expect("count by repo scope"),
        0,
        "and they stay out of the repo scope's results, which is the point of partitioning"
    );
}
