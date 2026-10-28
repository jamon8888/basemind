//! End-to-end document indexing benchmark.
//!
//! The harness deliberately calls the public scanner seam. A manifest is a
//! newline-separated list of relative paths; blank lines and `#` comments are
//! ignored. The same command can measure a cold scan, a warm scan, and an
//! incremental scan over selected manifest paths.
//!
//! ```text
//! cargo run --release --features documents --example document_benchmark -- \
//!   /path/to/workspace --manifest /path/to/manifest.txt --mode all
//! ```
#![cfg(feature = "documents")]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Result, anyhow};
use basemind::config::ConfigV1;
use basemind::scanner::{
    EmbedMode, FileResult, ScanCancel, ScanObserver, ScanReport, ScanSource, scan_paths_with_observer,
    scan_with_observer,
};
use basemind::store::{Store, VIEW_WORKING};
use serde_json::{Value, json};

#[derive(Clone, Copy)]
enum Mode {
    Cold,
    Warm,
    Incremental,
    All,
}

struct FirstResultObserver {
    started: Instant,
    first_result_ms: Option<u128>,
}

impl FirstResultObserver {
    fn new(started: Instant) -> Self {
        Self {
            started,
            first_result_ms: None,
        }
    }
}

impl ScanObserver for FirstResultObserver {
    fn on_file(&mut self, _result: FileResult) {
        if self.first_result_ms.is_none() {
            self.first_result_ms = Some(self.started.elapsed().as_millis());
        }
    }
}

impl Mode {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "cold" => Ok(Self::Cold),
            "warm" => Ok(Self::Warm),
            "incremental" => Ok(Self::Incremental),
            "all" => Ok(Self::All),
            other => Err(anyhow!(
                "unknown mode {other}; expected cold, warm, incremental, or all"
            )),
        }
    }
}

fn manifest(root: &Path, path: Option<&Path>) -> Result<Vec<PathBuf>> {
    let Some(path) = path else {
        return Ok(Vec::new());
    };
    let text = fs::read_to_string(path).map_err(|error| anyhow!("read manifest: {error}"))?;
    let mut files = Vec::new();
    for (line, raw) in text.lines().enumerate() {
        let item = raw.trim();
        if item.is_empty() || item.starts_with('#') {
            continue;
        }
        let relative = PathBuf::from(item);
        let absolute = root.join(&relative);
        if !absolute.is_file() {
            return Err(anyhow!(
                "manifest line {} is not a file: {}",
                line + 1,
                relative.display()
            ));
        }
        files.push(absolute);
    }
    files.sort();
    files.dedup();
    Ok(files)
}

fn json_report(
    mode: &str,
    report: &ScanReport,
    manifest_files: usize,
    manifest_bytes: u64,
    first_result_ms: Option<u128>,
    config: &ConfigV1,
    rss_before: Option<u64>,
    rss_after: Option<u64>,
) -> Value {
    let stats = report.stats;
    let total_seconds = report.timings.total_ms as f64 / 1_000.0;
    json!({
        "mode": mode,
        "manifest_files": manifest_files,
        "manifest_bytes": manifest_bytes,
        "cancelled": report.cancelled,
        "first_result_ms": first_result_ms,
        "rss_before_bytes": rss_before,
        "rss_after_bytes": rss_after,
        "peak_rss_bytes": basemind::sysres::sample().peak_bytes,
        "throughput": {
            "documents_per_second": if total_seconds > 0.0 { stats.docs_indexed as f64 / total_seconds } else { 0.0 },
            "bytes_per_second": if total_seconds > 0.0 { manifest_bytes as f64 / total_seconds } else { 0.0 },
            "chunks_per_second": if total_seconds > 0.0 { stats.docs_chunks as f64 / total_seconds } else { 0.0 },
        },
        "configuration": {
            "embedding_preset": config.documents.embedding_preset,
            "onnx_provider": config.resources.onnx_provider,
            "precision": "fp32",
            "scan_threads": config.resources.scan_threads,
            "embed_threads": config.resources.embed_threads,
            "embed_batch_size": config.resources.embed_batch_size,
            "max_concurrent_documents": config.resources.max_concurrent_documents,
        },
        "timings_ms": {
            "candidates": report.timings.candidates_ms,
            "extraction": report.timings.extraction_ms,
            "purge": report.timings.purge_ms,
            "flush": report.timings.flush_ms,
            "resolve": report.timings.resolve_ms,
            "lanes": report.timings.lanes_ms,
            "total": report.timings.total_ms,
        },
        "stats": {
            "scanned": stats.scanned,
            "updated": stats.updated,
            "skipped_unchanged": stats.skipped_unchanged,
            "docs_indexed": stats.docs_indexed,
            "reused_doc_extraction": stats.reused_doc_extraction,
            "docs_degraded": stats.docs_degraded,
            "doc_timeouts": stats.doc_timeouts,
            "docs_tombstoned": stats.docs_tombstoned,
            "docs_deferred": stats.docs_deferred,
            "read_failed": stats.read_failed,
            "extract_failed": stats.extract_failed,
            "parse_timeouts": stats.parse_timeouts,
            "removed": stats.removed,
        },
    })
}

fn usage() -> &'static str {
    "usage: document_benchmark <root> [--manifest FILE] [--mode cold|warm|incremental|all]"
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = args.first().map(PathBuf::from).ok_or_else(|| anyhow!("{}", usage()))?;
    let mut manifest_path = None;
    let mut mode = Mode::All;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--manifest" => {
                i += 1;
                manifest_path = Some(PathBuf::from(args.get(i).ok_or_else(|| anyhow!("{}", usage()))?));
            }
            "--mode" => {
                i += 1;
                mode = Mode::parse(args.get(i).ok_or_else(|| anyhow!("{}", usage()))?)?;
            }
            other => return Err(anyhow!("unknown argument {other}\n{}", usage())),
        }
        i += 1;
    }

    if !root.is_dir() {
        return Err(anyhow!("root is not a directory: {}", root.display()));
    }
    basemind::store::init_isolated_cache();
    let manifest_files = manifest(&root, manifest_path.as_deref())?;
    let manifest_bytes: u64 = manifest_files
        .iter()
        .filter_map(|path| fs::metadata(path).ok().map(|meta| meta.len()))
        .sum();
    let mut config = ConfigV1::with_defaults();
    config.documents.enabled = true;
    let mut store = Store::open(&root, VIEW_WORKING).map_err(|error| anyhow!("open store: {error}"))?;
    let mut runs = Vec::new();

    let mut full_scan = |label: &str, store: &mut Store| -> Result<()> {
        let started = Instant::now();
        let rss_before = basemind::sysres::sample().current_bytes;
        let mut observer = FirstResultObserver::new(started);
        let result = if manifest_files.is_empty() {
            scan_with_observer(
                &root,
                store,
                &config,
                ScanSource::WorkingTree,
                EmbedMode::Inline,
                &ScanCancel::new(),
                &mut observer,
            )
        } else {
            scan_paths_with_observer(
                &root,
                store,
                &config,
                &manifest_files,
                EmbedMode::Inline,
                &ScanCancel::new(),
                &mut observer,
            )
        }
        .map_err(|error| anyhow!("{label} scan: {error}"))?;
        let mut value = json_report(
            label,
            &result,
            manifest_files.len(),
            manifest_bytes,
            observer.first_result_ms,
            &config,
            rss_before,
            basemind::sysres::sample().current_bytes,
        );
        value["wall_clock_ms"] = json!(started.elapsed().as_millis());
        runs.push(value);
        Ok(())
    };

    let mut incremental_scan = |paths: &[PathBuf], store: &mut Store| -> Result<()> {
        let started = Instant::now();
        let rss_before = basemind::sysres::sample().current_bytes;
        let mut observer = FirstResultObserver::new(started);
        let result = scan_paths_with_observer(
            &root,
            store,
            &config,
            paths,
            EmbedMode::Inline,
            &ScanCancel::new(),
            &mut observer,
        )
        .map_err(|error| anyhow!("incremental scan: {error}"))?;
        let selected_bytes: u64 = paths
            .iter()
            .filter_map(|path| fs::metadata(path).ok().map(|meta| meta.len()))
            .sum();
        let mut value = json_report(
            "incremental",
            &result,
            paths.len(),
            selected_bytes,
            observer.first_result_ms,
            &config,
            rss_before,
            basemind::sysres::sample().current_bytes,
        );
        value["wall_clock_ms"] = json!(started.elapsed().as_millis());
        runs.push(value);
        Ok(())
    };

    match mode {
        Mode::Cold => full_scan("cold", &mut store)?,
        Mode::Warm => {
            full_scan("warm", &mut store)?;
        }
        Mode::Incremental => {
            let changed = if manifest_files.is_empty() {
                return Err(anyhow!("incremental mode requires --manifest with at least one file"));
            } else {
                vec![manifest_files[0].clone()]
            };
            incremental_scan(&changed, &mut store)?;
        }
        Mode::All => {
            full_scan("cold", &mut store)?;
            full_scan("warm", &mut store)?;
            if !manifest_files.is_empty() {
                incremental_scan(&[manifest_files[0].clone()], &mut store)?;
            }
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "root": root,
            "manifest_files": manifest_files.len(),
            "runs": runs,
            "configuration": {
                "embedding_preset": config.documents.embedding_preset,
                "onnx_provider": config.resources.onnx_provider,
                "scan_threads": config.resources.scan_threads,
                "embed_threads": config.resources.embed_threads,
                "embed_batch_size": config.resources.embed_batch_size,
                "max_concurrent_documents": config.resources.max_concurrent_documents,
            },
        }))
        .map_err(|error| anyhow!("serialize benchmark: {error}"))?
    );
    Ok(())
}
