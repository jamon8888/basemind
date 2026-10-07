//! Unit tests for the documents-tier row and schema shape.
//!
//! Split out of `mod.rs` by the 1000-line module cap (`tests/max_lines.rs`), and for the same
//! reason `scanner_docs` keeps its tests beside it rather than in the module body. Included as a
//! `#[path]` module so it can still reach the private `build_documents_batch`.

use super::*;

/// Phase 0 of spec 0012: `[documents] embed = false` has to be able to store a row at all.
///
/// Before the nullable `embedding` column this could not be expressed — the column was
/// non-nullable, so a lexical-only store could not be created, which is why `flush_document_batches`
/// refused to write anything for one.
#[test]
fn documents_embedding_column_is_nullable_for_a_lexical_only_store() {
    use crate::lance::schema::documents_schema;
    let schema = documents_schema(384);
    let field = schema.field_with_name("embedding").expect("embedding column exists");
    assert!(
        field.is_nullable(),
        "embedding must be nullable: `embed = false` stores a null vector, not no row"
    );
    // The dimension still comes from the preset. A lexical-only store is not a dim-less store:
    // `memory` and `code_chunks` are created in the same connection and both need a concrete
    // dimension, so `embed = false` means "do not run the embedder", not "no model configured".
    assert!(matches!(
        field.data_type(),
        arrow_schema::DataType::FixedSizeList(_, size) if *size == 384
    ));
}

/// The regression this ticket exists for: a vectorless row used to be an error
/// ("documents row embedding dim 0 does not match store dim 384"), so the flush dropped it.
#[test]
fn a_vectorless_document_row_builds_with_a_null_embedding() {
    let rows = vec![DocumentRow {
        scope: "repo:x".to_string(),
        path: "safe/a.md".to_string(),
        chunk_idx: 0,
        mime_type: "text/markdown".to_string(),
        text: "clause de résiliation".to_string(),
        byte_start: 0,
        byte_end: 21,
        rehydration_ref: None,
        embedding: Vec::new(),
    }];
    let batch = build_documents_batch(384, &rows).expect("a lexical-only row is not an error");
    let column = batch.column_by_name("embedding").expect("embedding column");
    assert_eq!(
        column.null_count(),
        1,
        "the vector column carries a null, not an empty list"
    );
    assert_eq!(batch.num_rows(), 1, "the row is written; text included");
}

/// The nullable column must not become a hole. A row that *claims* a vector of the wrong length
/// would misalign the index silently, which is worse than an error.
#[test]
fn a_wrong_length_vector_is_still_rejected() {
    let rows = vec![DocumentRow {
        scope: "repo:x".to_string(),
        path: "safe/a.md".to_string(),
        chunk_idx: 0,
        mime_type: "text/markdown".to_string(),
        text: "t".to_string(),
        byte_start: 0,
        byte_end: 1,
        rehydration_ref: None,
        embedding: vec![0.0_f32; 7],
    }];
    assert!(build_documents_batch(384, &rows).is_err());
}
