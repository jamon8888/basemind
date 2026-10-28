//! The end-to-end scan seam exposes phase timings for benchmark tooling.

use std::fs;

use basemind::config::ConfigV1;
use basemind::scanner::{EmbedMode, ScanSource, scan};
use basemind::store::{Store, VIEW_WORKING};

#[test]
fn full_scan_reports_nonzero_phase_and_total_timings() {
    basemind::store::init_isolated_cache();
    let root = tempfile::tempdir().expect("tempdir");
    fs::write(root.path().join("module.py"), "def answer():\n    return 42\n").expect("write fixture");

    let mut config = ConfigV1::with_defaults();
    config.documents.embed = false;
    config.code_search.embed = false;
    let mut store = Store::open(root.path(), VIEW_WORKING).expect("store");

    let report = scan(
        root.path(),
        &mut store,
        &config,
        ScanSource::WorkingTree,
        EmbedMode::Inline,
    )
    .expect("scan");

    assert!(report.timings.total_ms >= report.timings.candidates_ms);
    assert!(report.timings.extraction_ms <= report.timings.total_ms);
    assert!(report.timings.flush_ms <= report.timings.total_ms);
    assert!(report.stats.scanned >= 1);
}
