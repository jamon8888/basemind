//! Document persistence lane extracted from the main scanner module.

#[cfg(feature = "documents")]
use std::time::Instant;

use crate::config::Config;
use crate::scan_metrics::ScanStageMetrics;
use crate::scanner::ScanObserver;
use crate::scanner_drive::PendingDocBatchOpt;
use crate::store::Store;

#[cfg(feature = "documents")]
use crate::scanner_docs::flush_document_batches;

/// Push buffered document batches into LanceDB and notify the observer once rows are queryable.
pub(crate) fn flush_doc_batches_if_any(
    store: &mut Store,
    config: &Config,
    batches: Vec<PendingDocBatchOpt>,
    metrics: &ScanStageMetrics,
    observer: &mut dyn ScanObserver,
) {
    #[cfg(feature = "documents")]
    {
        if batches.is_empty() {
            return;
        }
        let persistence_started = Instant::now();
        crate::scanner_doc_links::flush_doc_links(store, config, &batches);
        let inserted = flush_document_batches(store, batches, &config.documents.embedding_preset);
        crate::scanner_docs::ensure_document_fts_index(store, config);
        metrics.add_persistence(persistence_started.elapsed());
        if inserted > 0 {
            observer.on_searchable();
        }
    }
    #[cfg(not(feature = "documents"))]
    {
        let _ = (store, config, batches, metrics, observer);
    }
}
