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
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "cold" => Ok(Self::Cold),
            "warm" => Ok(Self::Warm),
            "incremental" => Ok(Self::Incremental),
            "all" => Ok(Self::All),
            other => Err(format!(
                "unknown mode {other}; expected cold, warm, incremental, or all"
            )),
        }
    }
}

fn manifest(root: &Path, path: Option<&Path>) -> Result<Vec<PathBuf>, String> {
    let Some(path) = path else {
        return Ok(Vec::new());
    };
    let text = fs::read_to_string(path).map_err(|error| format!("read manifest: {error}"))?;
    let mut files = Vec::new();
    for (line, raw) in text.lines().enumerate() {
        let item = raw.trim();
        if item.is_empty() || item.starts_with('#') {
            continue;
        }
        let relative = PathBuf::from(item);
        let absolute = root.join(&relative);
        if !absolute.is_file() {
            return Err(format!(
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

fn report(mode: &str, report: &ScanReport, manifest_files: usize, first_result_ms: Option<u128>) -> Value {
    let stats = report.stats;
    json!({
        "mode": mode,
        "manifest_files": manifest_files,
        "cancelled": report.cancelled,
        "first_result_ms": first_result_ms,
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

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = args.first().map(PathBuf::from).ok_or_else(|| usage().to_string())?;
    let mut manifest_path = None;
    let mut mode = Mode::All;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--manifest" => {
                i += 1;
                manifest_path = Some(PathBuf::from(args.get(i).ok_or_else(|| usage().to_string())?));
            }
            "--mode" => {
                i += 1;
                mode = Mode::parse(args.get(i).ok_or_else(|| usage().to_string())?)?;
            }
            other => return Err(format!("unknown argument {other}\n{}", usage())),
        }
        i += 1;
    }

    if !root.is_dir() {
        return Err(format!("root is not a directory: {}", root.display()));
    }
    basemind::store::init_isolated_cache();
    let manifest_files = manifest(&root, manifest_path.as_deref())?;
    let mut config = ConfigV1::with_defaults();
    config.documents.enabled = true;
    let mut store = Store::open(&root, VIEW_WORKING).map_err(|error| format!("open store: {error}"))?;
    let mut runs = Vec::new();

    let mut full_scan = |label: &str, store: &mut Store| -> Result<(), String> {
        let started = Instant::now();
        let mut observer = FirstResultObserver::new(started);
        let result = scan_with_observer(
            &root,
            store,
            &config,
            ScanSource::WorkingTree,
            EmbedMode::Inline,
            &ScanCancel::new(),
            &mut observer,
        )
        .map_err(|error| format!("{label} scan: {error}"))?;
        let mut value = report(label, &result, manifest_files.len(), observer.first_result_ms);
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
                return Err("incremental mode requires --manifest with at least one file".to_string());
            } else {
                manifest_files.clone()
            };
            let started = Instant::now();
            let mut observer = FirstResultObserver::new(started);
            let result = scan_paths_with_observer(
                &root,
                &mut store,
                &config,
                &changed,
                EmbedMode::Inline,
                &ScanCancel::new(),
                &mut observer,
            )
            .map_err(|error| format!("incremental scan: {error}"))?;
            let mut value = report("incremental", &result, manifest_files.len(), observer.first_result_ms);
            value["wall_clock_ms"] = json!(started.elapsed().as_millis());
            runs.push(value);
        }
        Mode::All => {
            full_scan("cold", &mut store)?;
            full_scan("warm", &mut store)?;
            if !manifest_files.is_empty() {
                let started = Instant::now();
                let mut observer = FirstResultObserver::new(started);
                let result = scan_paths_with_observer(
                    &root,
                    &mut store,
                    &config,
                    &manifest_files,
                    EmbedMode::Inline,
                    &ScanCancel::new(),
                    &mut observer,
                )
                .map_err(|error| format!("incremental scan: {error}"))?;
                let mut value = report("incremental", &result, manifest_files.len(), observer.first_result_ms);
                value["wall_clock_ms"] = json!(started.elapsed().as_millis());
                runs.push(value);
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
        .map_err(|error| format!("serialize benchmark: {error}"))?
    );
    Ok(())
}
