//! Regression for basemind#30: the scan hung forever on a document embed pass.
//!
//! `process_doc` holds a document slot (`max_concurrent_documents`, default 1) across extraction,
//! and extraction embeds through the dedicated embed pool. `rayon::ThreadPool::install` from a
//! worker of a *different* pool does not park the caller: while it waits it keeps running jobs of
//! its own pool. So the scan worker that already holds the only slot started the next document,
//! which asked for a slot the same thread still held, and waited on it forever.
//!
//! No embedding model is needed: the guard is the shape (slot held, embed-pool call, another
//! document queued on the same scan worker).
#![cfg(feature = "intelligence")]

use std::sync::mpsc;
use std::time::Duration;

use basemind::backpressure::acquire_doc_slot;
use basemind::embeddings::on_embed_pool;
use rayon::prelude::*;

#[test]
fn embedding_while_holding_the_only_doc_slot_does_not_deadlock() {
    let (done, finished) = mpsc::channel();
    // Detached on purpose: if the bug is back this thread never returns, and the test must fail
    // on the timeout below instead of hanging the whole suite.
    std::thread::spawn(move || {
        // One scan worker and two documents: while the first is embedding, the only worker has
        // the second one queued behind it.
        let scan = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .expect("scan pool");
        scan.install(|| {
            (0..2).into_par_iter().for_each(|_| {
                let _slot = acquire_doc_slot(1);
                on_embed_pool(2, || ());
            });
        });
        let _ = done.send(());
    });

    assert!(
        finished.recv_timeout(Duration::from_secs(20)).is_ok(),
        "a scan worker holding the only document slot deadlocked while embedding"
    );
}
