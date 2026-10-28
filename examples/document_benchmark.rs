//! End-to-end document indexing benchmark.
//!
//! The harness deliberately calls the public scanner seam. A manifest is a
//! newline-separated list of relative paths; blank lines and `#` comments are
//! ignored. The same command can measure a cold scan, a warm scan, and an
//! incremental scan over selected manifest paths.
//! Pass `--synthetic-manifest` to add a checked-in/generated fixture set (PDF,
//! DOCX, PPTX, XLSX, and representative text files) to the private corpus.
//!
//! ```text
//! cargo run --release --features documents --example document_benchmark -- \
//!   /path/to/workspace --manifest /path/to/manifest.txt --mode all
//! ```
#![cfg(feature = "documents")]

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Result, anyhow};
use basemind::config::ConfigV1;
use basemind::embeddings::SharedEmbedder;
use basemind::scanner::{
    EmbedMode, FileResult, ScanCancel, ScanObserver, ScanReport, ScanSource, scan_paths_with_observer,
    scan_with_observer, working_tree_scope,
};
use basemind::store::{Store, VIEW_WORKING};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Clone, Copy)]
enum Mode {
    Cold,
    Warm,
    Incremental,
    All,
}

#[derive(Debug, Deserialize)]
struct QualityCase {
    query: Option<String>,
    #[serde(default)]
    expected_paths: Vec<String>,
    #[serde(default)]
    k: Option<usize>,
    #[serde(default)]
    ner: Vec<NerExpectation>,
}

#[derive(Debug, Deserialize)]
struct NerExpectation {
    path: String,
    #[serde(default)]
    required_categories: Vec<String>,
    #[serde(default)]
    required_texts: Vec<String>,
    #[serde(default)]
    forbidden_categories: Vec<String>,
    #[serde(default)]
    forbidden_texts: Vec<String>,
}

struct FirstResultObserver {
    started: Instant,
    first_searchable_ms: Option<u128>,
}

impl FirstResultObserver {
    fn new(started: Instant) -> Self {
        Self {
            started,
            first_searchable_ms: None,
        }
    }
}

impl ScanObserver for FirstResultObserver {
    fn on_file(&mut self, _result: FileResult) {}

    fn on_searchable(&mut self) {
        if self.first_searchable_ms.is_none() {
            self.first_searchable_ms = Some(self.started.elapsed().as_millis());
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
        let absolute = if absolute.is_file() {
            absolute
        } else if let Some(parent) = path.parent() {
            // Fixture manifests are commonly emitted beside their files. Resolve a missing entry
            // beside the manifest, while still requiring the final path to live under `root`.
            root.join(parent).join(&relative)
        } else {
            absolute
        };
        if !absolute.starts_with(root) {
            return Err(anyhow!(
                "manifest line {} is outside benchmark root: {}",
                line + 1,
                absolute.display()
            ));
        }
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
    peak_rss_before: Option<u64>,
) -> Value {
    let stats = report.stats;
    let total_seconds = report.timings.total_ms as f64 / 1_000.0;
    json!({
        "mode": mode,
        "manifest_files": manifest_files,
        "manifest_bytes": manifest_bytes,
        "cancelled": report.cancelled,
        "first_result_ms": first_result_ms,
        "first_searchable_result_ms": first_result_ms,
        "rss_before_bytes": rss_before,
        "rss_after_bytes": rss_after,
        "peak_rss_bytes": basemind::sysres::sample().peak_bytes,
        "peak_rss_before_bytes": peak_rss_before,
        "peak_rss_delta_bytes": basemind::sysres::sample()
            .peak_bytes
            .zip(peak_rss_before)
            .map(|(after, before)| after.saturating_sub(before)),
        "throughput": {
            "documents_per_second": if total_seconds > 0.0 { stats.docs_indexed as f64 / total_seconds } else { 0.0 },
            "bytes_per_second": if total_seconds > 0.0 { stats.docs_bytes as f64 / total_seconds } else { 0.0 },
            "chunks_per_second": if total_seconds > 0.0 { stats.docs_chunks as f64 / total_seconds } else { 0.0 },
            "tokens_per_second": if total_seconds > 0.0 { stats.docs_tokens as f64 / total_seconds } else { 0.0 },
        },
        "configuration": {
            "embedding_preset": config.documents.embedding_preset,
            "embedding_model": config.documents.embedding_preset,
            "onnx_provider": config.resources.onnx_provider,
            "precision": "fp32",
            "ner_enabled": config.documents.ner.enabled,
            "ner_backend": format!("{:?}", config.documents.ner.backend),
            "ner_model": config.documents.ner.model,
            "scan_threads": config.resources.scan_threads,
            "embed_threads": config.resources.embed_threads,
            "embed_batch_size": config.resources.embed_batch_size,
            "max_concurrent_documents": config.resources.max_concurrent_documents,
        },
        "timings_ms": {
            "candidates": report.timings.candidates_ms,
            "extraction": report.timings.extraction_ms,
            "chunking": report.timings.chunking_ms,
            "ner": report.timings.ner_ms,
            "embedding": report.timings.embedding_ms,
            "persistence": report.timings.persistence_ms,
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
            "docs_chunks": stats.docs_chunks,
            "docs_tokens": stats.docs_tokens,
            "docs_bytes": stats.docs_bytes,
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

fn read_quality_manifest(path: &Path) -> Result<Vec<QualityCase>> {
    let text = fs::read_to_string(path).map_err(|error| anyhow!("read quality manifest: {error}"))?;
    text.lines()
        .enumerate()
        .filter_map(|(line, raw)| {
            let raw = raw.trim();
            if raw.is_empty() || raw.starts_with('#') {
                return None;
            }
            Some(serde_json::from_str(raw).map_err(|error| anyhow!("quality manifest line {}: {error}", line + 1)))
        })
        .collect()
}

fn mutate_synthetic_incremental(path: &Path) -> Result<()> {
    if path.extension().and_then(|ext| ext.to_str()) != Some("txt") {
        return Err(anyhow!(
            "--mutate-incremental requires a .txt fixture; edit {} between runs instead",
            path.display()
        ));
    }
    let mut bytes = fs::read(path).map_err(|error| anyhow!("read incremental fixture: {error}"))?;
    let marker = b"Basemind synthetic incremental update\n";
    if !bytes.ends_with(marker) {
        bytes.extend_from_slice(marker);
        fs::write(path, bytes).map_err(|error| anyhow!("write incremental fixture: {error}"))?;
    }
    Ok(())
}

fn quality_report(
    root: &Path,
    store: &mut Store,
    config: &ConfigV1,
    manifest_path: &Path,
    baseline_path: Option<&Path>,
    quality_tolerance: f64,
) -> Result<Value> {
    let cases = read_quality_manifest(manifest_path)?;
    let embedder = SharedEmbedder::load_with_provider(
        &config.documents.embedding_preset,
        config
            .resources
            .effective_embed_threads(config.documents.embed_max_threads),
        config.resources.embed_batch_size,
        config.resources.onnx_provider,
    )
    .map_err(|error| anyhow!("load quality embedder: {error}"))?;
    let scope = working_tree_scope(root);
    let lance = store
        .lance_or_open(embedder.dim(), &config.documents.embedding_preset)
        .map_err(|error| anyhow!("open quality vector store: {error}"))?
        .clone();
    let mut retrieval = Vec::new();
    let mut ner_required = 0usize;
    let mut ner_matched = 0usize;
    let mut ner_forbidden_hits = 0usize;
    let mut ner_invalid_spans = 0usize;
    for case in cases {
        if let Some(query) = case.query.as_deref()
            && !case.expected_paths.is_empty()
        {
            let vectors = embedder
                .embed_batch(&[query])
                .map_err(|error| anyhow!("embed quality query: {error}"))?;
            let k = case.k.unwrap_or(10).max(1);
            let hits = lance
                .search_documents(&scope, vectors.into_iter().next().unwrap_or_default(), k, None)
                .map_err(|error| anyhow!("run quality query: {error}"))?;
            let expected: BTreeSet<&str> = case.expected_paths.iter().map(String::as_str).collect();
            let mut seen = BTreeSet::new();
            let mut relevant = 0usize;
            let mut dcg = 0.0_f64;
            for (rank, hit) in hits.iter().enumerate() {
                if expected.contains(hit.path.as_str()) && seen.insert(hit.path.as_str()) {
                    relevant += 1;
                    dcg += 1.0 / ((rank + 2) as f64).log2();
                }
            }
            let ideal = (0..expected.len().min(k))
                .map(|rank| 1.0 / ((rank + 2) as f64).log2())
                .sum::<f64>();
            retrieval.push(json!({
                "query": query,
                "k": k,
                "expected": expected.len(),
                "retrieved_relevant": relevant,
                "recall": relevant as f64 / expected.len() as f64,
                "hit_at_1": hits.first().is_some_and(|hit| expected.contains(hit.path.as_str())),
                "ndcg": if ideal > 0.0 { dcg / ideal } else { 1.0 },
            }));
        }
        for expectation in case.ner {
            ner_required += expectation.required_categories.len() + expectation.required_texts.len();
            let Some(entry) = store.lookup_doc(&expectation.path) else {
                continue;
            };
            let Some(doc) = store
                .read_doc_by_hex(&entry.hash_hex)
                .map_err(|error| anyhow!("read NER blob: {error}"))?
            else {
                continue;
            };
            let categories: BTreeSet<String> = doc
                .entities
                .iter()
                .map(|entity| entity.category.to_lowercase())
                .collect();
            let texts: BTreeSet<String> = doc.entities.iter().map(|entity| entity.text.to_lowercase()).collect();
            ner_matched += expectation
                .required_categories
                .iter()
                .filter(|category| categories.contains(&category.to_lowercase()))
                .count();
            ner_matched += expectation
                .required_texts
                .iter()
                .filter(|text| texts.contains(&text.to_lowercase()))
                .count();
            ner_forbidden_hits += expectation
                .forbidden_categories
                .iter()
                .filter(|category| categories.contains(&category.to_lowercase()))
                .count();
            ner_forbidden_hits += expectation
                .forbidden_texts
                .iter()
                .filter(|text| texts.contains(&text.to_lowercase()))
                .count();
            ner_invalid_spans += doc
                .entities
                .iter()
                .filter(|entity| {
                    entity.start >= entity.end || usize::try_from(entity.end).unwrap_or(usize::MAX) > doc.content.len()
                })
                .count();
        }
    }
    let mean_recall = if retrieval.is_empty() {
        None
    } else {
        Some(
            retrieval
                .iter()
                .map(|row| row["recall"].as_f64().unwrap_or(0.0))
                .sum::<f64>()
                / retrieval.len() as f64,
        )
    };
    let mean_ndcg = if retrieval.is_empty() {
        None
    } else {
        Some(
            retrieval
                .iter()
                .map(|row| row["ndcg"].as_f64().unwrap_or(0.0))
                .sum::<f64>()
                / retrieval.len() as f64,
        )
    };
    let mut report = json!({
        "manifest": manifest_path,
        "retrieval": {
            "cases": retrieval.len(),
            "mean_recall": mean_recall,
            "mean_ndcg": mean_ndcg,
            "cases_detail": retrieval,
        },
        "ner": {
            "required_checks": ner_required,
            "matched_checks": ner_matched,
            "coverage": if ner_required > 0 { ner_matched as f64 / ner_required as f64 } else { 1.0 },
            "forbidden_hits": ner_forbidden_hits,
            "invalid_spans": ner_invalid_spans,
            "precision_proxy": if ner_matched + ner_forbidden_hits > 0 {
                ner_matched as f64 / (ner_matched + ner_forbidden_hits) as f64
            } else {
                1.0
            },
        },
    });
    if let Some(path) = baseline_path {
        let baseline_text = fs::read_to_string(path).map_err(|error| anyhow!("read quality baseline: {error}"))?;
        let baseline: Value =
            serde_json::from_str(&baseline_text).map_err(|error| anyhow!("parse quality baseline: {error}"))?;
        let baseline_quality = baseline.get("quality").unwrap_or(&baseline);
        let delta = |current: Option<f64>, key: &str| {
            current.and_then(|value| {
                baseline_quality
                    .pointer(key)
                    .and_then(Value::as_f64)
                    .map(|base| value - base)
            })
        };
        let recall_delta = delta(report["retrieval"]["mean_recall"].as_f64(), "/retrieval/mean_recall");
        let ndcg_delta = delta(report["retrieval"]["mean_ndcg"].as_f64(), "/retrieval/mean_ndcg");
        let ner_coverage_delta = delta(report["ner"]["coverage"].as_f64(), "/ner/coverage");
        let within_tolerance = [recall_delta, ndcg_delta, ner_coverage_delta]
            .into_iter()
            .flatten()
            .all(|value| value >= -quality_tolerance);
        report["baseline_comparison"] = json!({
            "path": path,
            "tolerance": quality_tolerance,
            "within_tolerance": within_tolerance,
            "recall_delta": recall_delta,
            "ndcg_delta": ndcg_delta,
            "ner_coverage_delta": ner_coverage_delta,
        });
        if !within_tolerance {
            return Err(anyhow!(
                "quality regression exceeds tolerance ({quality_tolerance:.4}) compared with {}",
                path.display()
            ));
        }
    }
    Ok(report)
}

fn usage() -> &'static str {
    "usage: document_benchmark <root> [--manifest FILE] [--synthetic-manifest FILE] [--incremental-file PATH] [--mutate-incremental] [--quality-manifest FILE] [--quality-baseline FILE] [--quality-tolerance FRACTION] [--mode cold|warm|incremental|all]"
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = args.first().map(PathBuf::from).ok_or_else(|| anyhow!("{}", usage()))?;
    let mut manifest_path = None;
    let mut synthetic_manifest_path = None;
    let mut quality_manifest_path = None;
    let mut quality_baseline_path = None;
    let mut quality_tolerance = 0.01_f64;
    let mut incremental_file = None;
    let mut mutate_incremental = false;
    let mut mode = Mode::All;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--manifest" => {
                i += 1;
                manifest_path = Some(PathBuf::from(args.get(i).ok_or_else(|| anyhow!("{}", usage()))?));
            }
            "--synthetic-manifest" => {
                i += 1;
                synthetic_manifest_path = Some(PathBuf::from(args.get(i).ok_or_else(|| anyhow!("{}", usage()))?));
            }
            "--quality-manifest" => {
                i += 1;
                quality_manifest_path = Some(PathBuf::from(args.get(i).ok_or_else(|| anyhow!("{}", usage()))?));
            }
            "--quality-baseline" => {
                i += 1;
                quality_baseline_path = Some(PathBuf::from(args.get(i).ok_or_else(|| anyhow!("{}", usage()))?));
            }
            "--quality-tolerance" => {
                i += 1;
                quality_tolerance = args
                    .get(i)
                    .ok_or_else(|| anyhow!("{}", usage()))?
                    .parse::<f64>()
                    .map_err(|error| anyhow!("invalid quality tolerance: {error}"))?;
                if !quality_tolerance.is_finite() || quality_tolerance < 0.0 {
                    return Err(anyhow!("quality tolerance must be a finite non-negative fraction"));
                }
            }
            "--incremental-file" => {
                i += 1;
                incremental_file = Some(PathBuf::from(args.get(i).ok_or_else(|| anyhow!("{}", usage()))?));
            }
            "--mutate-incremental" => mutate_incremental = true,
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
    let mut manifest_files = manifest(&root, manifest_path.as_deref())?;
    let private_manifest_files = manifest_files.len();
    let discovered_synthetic = synthetic_manifest_path.or_else(|| {
        [
            root.join("fixtures/synthetic-manifest.txt"),
            root.join("synthetic-manifest.txt"),
        ]
        .into_iter()
        .find(|path| path.is_file())
    });
    let synthetic_manifest_files = manifest(&root, discovered_synthetic.as_deref())?;
    manifest_files.extend(synthetic_manifest_files);
    manifest_files.sort();
    manifest_files.dedup();
    let manifest_bytes: u64 = manifest_files
        .iter()
        .filter_map(|path| fs::metadata(path).ok().map(|meta| meta.len()))
        .sum();
    let mut config = ConfigV1::with_defaults();
    config.documents.enabled = true;
    let cache_before = root.join(".basemind/views/working/index.msgpack").is_file();
    let mut store = Store::open(&root, VIEW_WORKING).map_err(|error| anyhow!("open store: {error}"))?;
    let mut runs = Vec::new();
    let changed_file = incremental_file
        .map(|path| root.join(path))
        .or_else(|| {
            manifest_files
                .iter()
                .find(|path| path.file_name().is_some_and(|name| name == "synthetic-incremental.txt"))
                .cloned()
        })
        .or_else(|| manifest_files.first().cloned());

    let should_mutate = mutate_incremental
        || changed_file
            .as_ref()
            .is_some_and(|path| path.file_name().is_some_and(|name| name == "synthetic-incremental.txt"));

    let mut full_scan = |label: &str, store: &mut Store| -> Result<()> {
        let started = Instant::now();
        let rss_before = basemind::sysres::sample().current_bytes;
        let peak_rss_before = basemind::sysres::sample().peak_bytes;
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
            observer.first_searchable_ms,
            &config,
            rss_before,
            basemind::sysres::sample().current_bytes,
            peak_rss_before,
        );
        value["wall_clock_ms"] = json!(started.elapsed().as_millis());
        value["cache_before_run"] = json!(cache_before);
        runs.push(value);
        Ok(())
    };

    let mut incremental_scan = |paths: &[PathBuf], store: &mut Store| -> Result<()> {
        let started = Instant::now();
        let rss_before = basemind::sysres::sample().current_bytes;
        let peak_rss_before = basemind::sysres::sample().peak_bytes;
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
            observer.first_searchable_ms,
            &config,
            rss_before,
            basemind::sysres::sample().current_bytes,
            peak_rss_before,
        );
        value["wall_clock_ms"] = json!(started.elapsed().as_millis());
        value["cache_before_run"] = json!(cache_before);
        runs.push(value);
        Ok(())
    };

    match mode {
        Mode::Cold => full_scan("cold", &mut store)?,
        Mode::Warm => {
            full_scan("warm", &mut store)?;
        }
        Mode::Incremental => {
            let changed = changed_file
                .clone()
                .map(|path| vec![path])
                .ok_or_else(|| anyhow!("incremental mode requires --manifest or --incremental-file"))?;
            if should_mutate {
                mutate_synthetic_incremental(&changed[0])?;
            }
            incremental_scan(&changed, &mut store)?;
        }
        Mode::All => {
            full_scan("cold", &mut store)?;
            full_scan("warm", &mut store)?;
            if let Some(path) = changed_file {
                if should_mutate {
                    mutate_synthetic_incremental(&path)?;
                }
                incremental_scan(&[path], &mut store)?;
            }
        }
    }
    let quality = quality_manifest_path
        .as_deref()
        .map(|path| {
            quality_report(
                &root,
                &mut store,
                &config,
                path,
                quality_baseline_path.as_deref(),
                quality_tolerance,
            )
        })
        .transpose()?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "root": root,
            "manifest_files": manifest_files.len(),
            "private_manifest_files": private_manifest_files,
            "synthetic_manifest_files": manifest_files.len().saturating_sub(private_manifest_files),
            "runs": runs,
            "configuration": {
                "embedding_preset": config.documents.embedding_preset,
                "embedding_model": config.documents.embedding_preset,
                "onnx_provider": config.resources.onnx_provider,
                "scan_threads": config.resources.scan_threads,
                "embed_threads": config.resources.embed_threads,
                "embed_batch_size": config.resources.embed_batch_size,
                "max_concurrent_documents": config.resources.max_concurrent_documents,
                "ner_enabled": config.documents.ner.enabled,
                "ner_backend": format!("{:?}", config.documents.ner.backend),
                "ner_model": config.documents.ner.model,
            },
            "quality": quality,
        }))
        .map_err(|error| anyhow!("serialize benchmark: {error}"))?
    );
    Ok(())
}
