"""Verify the end-to-end document benchmark contract and optional release gates."""

from __future__ import annotations

import argparse
import json
import math
import sys
from pathlib import Path

TIMINGS = (
    "candidates",
    "extraction",
    "chunking",
    "ner",
    "embedding",
    "persistence",
    "purge",
    "flush",
    "resolve",
    "lanes",
    "total",
)
THROUGHPUT = (
    "documents_per_second",
    "bytes_per_second",
    "tokens_per_second",
    "chunks_per_second",
    "ner_documents_per_second",
    "ner_tokens_per_second",
)
STATS = (
    "scanned",
    "updated",
    "updated_with_warnings",
    "reused_extraction",
    "skipped_unchanged",
    "skipped_too_large",
    "skipped_non_utf8",
    "skipped_no_lang",
    "skipped_binary",
    "docs_indexed",
    "docs_chunks",
    "docs_tokens",
    "docs_bytes",
    "reused_doc_extraction",
    "reembedded_doc_embeddings",
    "docs_degraded",
    "docs_text_fallback",
    "doc_timeouts",
    "docs_tombstoned",
    "docs_deferred",
    "abandoned_extractions",
    "refreshed_extraction",
    "tier_migrated",
    "read_failed",
    "extract_failed",
    "parse_timeouts",
    "removed",
)
CONFIGURATION = (
    "embedding_preset",
    "onnx_provider",
    "precision",
    "ner_enabled",
    "ner_backend",
    "ner_model",
    "scan_threads",
    "embed_threads",
    "embed_batch_size",
    "max_concurrent_documents",
)


def require_object(value: object, path: str, errors: list[str]) -> dict:
    if not isinstance(value, dict):
        errors.append(f"{path} must be an object")
        return {}
    return value


def require_keys(obj: dict, keys: tuple[str, ...], path: str, errors: list[str]) -> None:
    for key in keys:
        if key not in obj:
            errors.append(f"missing {path}.{key}")


def number(obj: dict, key: str, path: str, errors: list[str]) -> float | None:
    value = obj.get(key)
    if not isinstance(value, (int, float)) or isinstance(value, bool) or not math.isfinite(float(value)):
        errors.append(f"{path}.{key} must be numeric")
        return None
    return float(value)


def non_negative_number(obj: dict, key: str, path: str, errors: list[str]) -> float | None:
    value = number(obj, key, path, errors)
    if value is not None and value < 0:
        errors.append(f"{path}.{key} must be non-negative")
    return value


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("report", type=Path, help="JSON emitted by document_benchmark")
    parser.add_argument("--require-modes", default="cold,warm,incremental")
    parser.add_argument("--warm-fraction", type=float, help="fail if warm total exceeds this fraction of cold")
    parser.add_argument("--max-rss-bytes", type=int, help="fail if any reported peak RSS exceeds this ceiling")
    parser.add_argument("--require-quality", action="store_true")
    parser.add_argument("--quality-tolerance", type=float, default=0.01)
    args = parser.parse_args()
    if args.warm_fraction is not None and args.warm_fraction < 0:
        parser.error("--warm-fraction must be non-negative")
    if args.max_rss_bytes is not None and args.max_rss_bytes < 0:
        parser.error("--max-rss-bytes must be non-negative")
    try:
        report = json.loads(args.report.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        print(f"error: cannot read benchmark report: {exc}", file=sys.stderr)
        return 2

    errors: list[str] = []
    root = require_object(report, "report", errors)
    runs = root.get("runs")
    if not isinstance(runs, list):
        errors.append("report.runs must be an array")
        runs = []
    by_mode: dict[str, dict] = {}
    for index, raw_run in enumerate(runs):
        run = require_object(raw_run, f"runs[{index}]", errors)
        mode = run.get("mode")
        if not isinstance(mode, str):
            errors.append(f"runs[{index}].mode must be a string")
            continue
        by_mode[mode] = run
        require_keys(
            run,
            ("first_searchable_result_ms", "peak_rss_bytes", "skip_reasons", "throughput", "timings_ms", "stats"),
            f"runs[{index}]",
            errors,
        )
        skip_reasons = run.get("skip_reasons")
        if not isinstance(skip_reasons, list):
            errors.append(f"runs[{index}].skip_reasons must be an array")
        else:
            for reason_index, raw_reason in enumerate(skip_reasons):
                reason = require_object(raw_reason, f"runs[{index}].skip_reasons[{reason_index}]", errors)
                if not isinstance(reason.get("path"), str) or not reason["path"]:
                    errors.append(f"runs[{index}].skip_reasons[{reason_index}].path must be a non-empty string")
                if not isinstance(reason.get("reason"), str) or not reason["reason"]:
                    errors.append(f"runs[{index}].skip_reasons[{reason_index}].reason must be a non-empty string")
        require_keys(
            require_object(run.get("timings_ms"), f"runs[{index}].timings_ms", errors),
            TIMINGS,
            f"runs[{index}].timings_ms",
            errors,
        )
        require_keys(
            require_object(run.get("throughput"), f"runs[{index}].throughput", errors),
            THROUGHPUT,
            f"runs[{index}].throughput",
            errors,
        )
        require_keys(
            require_object(run.get("stats"), f"runs[{index}].stats", errors), STATS, f"runs[{index}].stats", errors
        )
        first_searchable = run.get("first_searchable_result_ms")
        if first_searchable is not None and (
            not isinstance(first_searchable, (int, float))
            or isinstance(first_searchable, bool)
            or not math.isfinite(float(first_searchable))
        ):
            errors.append(f"runs[{index}].first_searchable_result_ms must be numeric or null")
        elif first_searchable is not None and first_searchable < 0:
            errors.append(f"runs[{index}].first_searchable_result_ms must be non-negative")
        for key in ("peak_rss_bytes", "peak_rss_before_bytes", "peak_rss_delta_bytes"):
            value = run.get(key)
            if value is not None and (not isinstance(value, int) or value < 0):
                errors.append(f"runs[{index}].{key} must be a non-negative integer or null")
        timings = require_object(run.get("timings_ms"), f"runs[{index}].timings_ms", errors)
        throughput = require_object(run.get("throughput"), f"runs[{index}].throughput", errors)
        stats = require_object(run.get("stats"), f"runs[{index}].stats", errors)
        for key in TIMINGS:
            non_negative_number(timings, key, f"runs[{index}].timings_ms", errors)
        for key in THROUGHPUT:
            non_negative_number(throughput, key, f"runs[{index}].throughput", errors)
        for key in STATS:
            value = stats.get(key)
            if not isinstance(value, int) or isinstance(value, bool) or value < 0:
                errors.append(f"runs[{index}].stats.{key} must be a non-negative integer")

    required_modes = tuple(mode.strip() for mode in args.require_modes.split(",") if mode.strip())
    for mode in required_modes:
        if mode not in by_mode:
            errors.append(f"missing required benchmark mode: {mode}")

    configuration = require_object(root.get("configuration"), "configuration", errors)
    require_keys(configuration, CONFIGURATION, "configuration", errors)
    if args.warm_fraction is not None and "cold" in by_mode and "warm" in by_mode:
        cold = require_object(by_mode["cold"].get("timings_ms"), "runs[cold].timings_ms", errors)
        warm = require_object(by_mode["warm"].get("timings_ms"), "runs[warm].timings_ms", errors)
        cold_total = number(cold, "total", "runs[cold].timings_ms", errors)
        warm_total = number(warm, "total", "runs[warm].timings_ms", errors)
        if cold_total is not None and warm_total is not None and warm_total > cold_total * args.warm_fraction:
            errors.append(
                f"warm total {warm_total:.0f} ms exceeds {args.warm_fraction:.1%} of cold {cold_total:.0f} ms"
            )
    if args.max_rss_bytes is not None:
        for mode, run in by_mode.items():
            peak = run.get("peak_rss_bytes")
            if isinstance(peak, int) and peak > args.max_rss_bytes:
                errors.append(f"{mode} peak RSS {peak} exceeds ceiling {args.max_rss_bytes}")

    quality = root.get("quality")
    if args.require_quality:
        quality_obj = require_object(quality, "quality", errors)
        retrieval = require_object(quality_obj.get("retrieval"), "quality.retrieval", errors)
        ner = require_object(quality_obj.get("ner"), "quality.ner", errors)
        require_keys(retrieval, ("mean_recall", "mean_ndcg"), "quality.retrieval", errors)
        require_keys(ner, ("coverage", "precision_proxy", "invalid_spans"), "quality.ner", errors)
        for key in ("mean_recall", "mean_ndcg"):
            value = number(retrieval, key, "quality.retrieval", errors)
            if value is not None and not 0 <= value <= 1:
                errors.append(f"quality.retrieval.{key} must be between 0 and 1")
        for key in ("coverage", "precision_proxy"):
            value = number(ner, key, "quality.ner", errors)
            if value is not None and not 0 <= value <= 1:
                errors.append(f"quality.ner.{key} must be between 0 and 1")
        invalid = ner.get("invalid_spans")
        if isinstance(invalid, int) and invalid != 0:
            errors.append(f"quality.ner.invalid_spans is {invalid}, expected 0")

    if errors:
        print("benchmark verification failed:", file=sys.stderr)
        print("\n".join(f"- {error}" for error in errors), file=sys.stderr)
        return 1
    print(f"verified {len(by_mode)} benchmark modes: {', '.join(sorted(by_mode))}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
