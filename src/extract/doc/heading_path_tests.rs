//! Tests for the persisted `heading_path` column.
//!
//! Split out of `doc.rs` by the 1000-line module cap (`tests/max_lines.rs`). Included as a
//! sibling module so `super::*` reaches the private `prepare_doc_chunk`.

use super::*;
use xberg::types::{HeadingContext, HeadingLevel};

/// The regression this column exists for: the breadcrumb used to be computed only when an
/// embed was requested, so `embed = false` produced an empty column — and `embed = false` is
/// exactly the store where the lexical lane is the only lane and the breadcrumb is the whole
/// structural signal. A test that only checked the `embed = true` path would pass while the
/// defect was untouched.
#[test]
fn heading_path_is_persisted_even_when_no_embed_is_requested() {
    let context = HeadingContext {
        headings: vec![
            HeadingLevel {
                level: 1,
                text: "Termination".to_string(),
            },
            HeadingLevel {
                level: 2,
                text: "Notice".to_string(),
            },
        ],
    };

    let (stored, retrieval) =
        prepare_doc_chunk("The lease may be terminated.".to_string(), 0, 28, Some(&context), false);

    assert_eq!(
        stored.heading_path, "# Termination > ## Notice",
        "the column is populated with no embed requested"
    );
    assert!(
        retrieval.is_none(),
        "and no dense input is allocated — that is still gated"
    );
}

/// A chunk with no heading context gets an empty column, not a placeholder. A guessed path would
/// be worse than none: it would look like structure the document does not have.
#[test]
fn heading_path_is_empty_without_heading_context() {
    let (stored, _) = prepare_doc_chunk("Body text.".to_string(), 0, 10, None, false);
    assert_eq!(stored.heading_path, "");

    let empty_context = HeadingContext { headings: Vec::new() };
    let (stored, _) = prepare_doc_chunk("Body text.".to_string(), 0, 10, Some(&empty_context), false);
    assert_eq!(stored.heading_path, "", "an empty heading stack is no heading context");
}

/// The stored breadcrumb is the *prefix* of the dense input, not the same string.
///
/// `render_heading_breadcrumb` appends the chunk body after `\n\n`, so the two spellings share
/// a prefix and diverge after it. Getting this wrong would store a body-prefixed blob in the
/// indexed column and inflate every hit's indexed text with its own content twice.
#[test]
fn stored_heading_path_is_the_prefix_of_the_dense_retrieval_input() {
    let content = "## Setup\n\nInstall the dependencies.".to_string();
    let context = HeadingContext {
        headings: vec![HeadingLevel {
            level: 2,
            text: "Setup".to_string(),
        }],
    };

    let (stored, retrieval) = prepare_doc_chunk(content.clone(), 10, 44, Some(&context), true);

    assert_eq!(stored.heading_path, "# Setup");
    let dense = retrieval.expect("embed requested => dense input");
    assert!(
        dense.starts_with(&stored.heading_path),
        "dense input `{dense}` must begin with the stored breadcrumb `{}`",
        stored.heading_path
    );
    assert!(
        dense.contains("Install the dependencies."),
        "and must still carry the body, which the column does not store"
    );
}
