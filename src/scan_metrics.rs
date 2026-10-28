//! Public scan measurements used by benchmark and operator tooling.

use std::time::Instant;

/// Wall-clock timings for one scan invocation, in milliseconds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanTimings {
    pub candidates_ms: u128,
    pub extraction_ms: u128,
    pub purge_ms: u128,
    pub flush_ms: u128,
    pub resolve_ms: u128,
    pub lanes_ms: u128,
    pub total_ms: u128,
}

pub(crate) fn elapsed_ms(started: Instant) -> u128 {
    started.elapsed().as_millis()
}
