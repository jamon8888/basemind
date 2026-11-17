//! Test-only helpers on [`LanceStore`], kept out of `mod.rs` so the 1000-line module cap
//! (`tests/max_lines.rs`) does not have to accommodate them.

use anyhow::{Context, Result};

use super::LanceStore;
use super::schema::DOCUMENTS_TABLE;

impl LanceStore {
    /// Count rows in the documents table, optionally filtered by a SQL predicate.
    ///
    /// Test-facing only. Nothing in the product needs a row count, and putting one on the public
    /// surface would be a promise to keep it accurate across compaction. It exists because the
    /// scope-mismatch defect it guards is *invisible* without it: a document whose rows were
    /// written under one scope and deleted under another still searches fine and still reports
    /// success — the duplicate only shows up as a count.
    #[cfg(any(test, feature = "test-support"))]
    pub fn count_documents(&self, filter: Option<&str>) -> Result<usize> {
        self.inner.rt().block_on(async {
            let table = self
                .inner
                .connection
                .open_table(DOCUMENTS_TABLE)
                .execute()
                .await
                .with_context(|| format!("open {DOCUMENTS_TABLE} table"))?;
            table
                .count_rows(filter.map(str::to_string))
                .await
                .context("count documents rows")
        })
    }
}
