//! Public scan measurements used by benchmark and operator tooling.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Wall-clock timings for one scan invocation, in milliseconds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanTimings {
    pub candidates_ms: u128,
    pub extraction_ms: u128,
    /// Time spent turning extracted content into scanner chunks.
    pub chunking_ms: u128,
    /// Time reported for the NER stage. The document backend may include NER inside its guarded
    /// extraction call; in that case this is the backend's extraction interval, not a second pass.
    pub ner_ms: u128,
    /// Time spent loading and invoking the embedding backend.
    pub embedding_ms: u128,
    /// Time spent writing document blobs and document vector rows.
    pub persistence_ms: u128,
    pub purge_ms: u128,
    pub flush_ms: u128,
    pub resolve_ms: u128,
    pub lanes_ms: u128,
    pub total_ms: u128,
}

/// Thread-safe stage accumulator shared by the parallel scanner workers for one scan invocation.
/// Nanoseconds are collected so sub-millisecond fixture runs still produce useful aggregate data.
#[derive(Debug, Default)]
pub(crate) struct ScanStageMetrics {
    pub(crate) extraction_ns: AtomicU64,
    pub(crate) chunking_ns: AtomicU64,
    pub(crate) ner_ns: AtomicU64,
    pub(crate) embedding_ns: AtomicU64,
    pub(crate) persistence_ns: AtomicU64,
}

impl ScanStageMetrics {
    pub(crate) fn add_extraction(&self, elapsed: Duration, ner_enabled: bool) {
        self.add(&self.extraction_ns, elapsed);
        if ner_enabled {
            // xberg performs configured NER during the guarded extraction call and does not expose
            // a separate timer. Preserve that fact instead of pretending the tiny entity mapping
            // loop is the model's inference time.
            self.add(&self.ner_ns, elapsed);
        }
    }

    pub(crate) fn add_chunking(&self, elapsed: Duration) {
        self.add(&self.chunking_ns, elapsed);
    }

    pub(crate) fn add_embedding(&self, elapsed: Duration) {
        self.add(&self.embedding_ns, elapsed);
    }

    pub(crate) fn add_persistence(&self, elapsed: Duration) {
        self.add(&self.persistence_ns, elapsed);
    }

    pub(crate) fn snapshot(&self) -> ScanTimings {
        ScanTimings {
            chunking_ms: nanos_to_ms(self.chunking_ns.load(Ordering::Relaxed)),
            ner_ms: nanos_to_ms(self.ner_ns.load(Ordering::Relaxed)),
            embedding_ms: nanos_to_ms(self.embedding_ns.load(Ordering::Relaxed)),
            persistence_ms: nanos_to_ms(self.persistence_ns.load(Ordering::Relaxed)),
            ..ScanTimings::default()
        }
    }

    fn add(&self, target: &AtomicU64, elapsed: Duration) {
        target.fetch_add(elapsed.as_nanos().min(u128::from(u64::MAX)) as u64, Ordering::Relaxed);
    }
}

pub(crate) fn apply_stage_timings(timings: &mut ScanTimings, metrics: &ScanStageMetrics) {
    let staged = metrics.snapshot();
    timings.chunking_ms = staged.chunking_ms;
    timings.ner_ms = staged.ner_ms;
    timings.embedding_ms = staged.embedding_ms;
    timings.persistence_ms = staged.persistence_ms;
}

fn nanos_to_ms(nanos: u64) -> u128 {
    u128::from(nanos) / 1_000_000
}

pub(crate) fn elapsed_ms(started: Instant) -> u128 {
    started.elapsed().as_millis()
}
