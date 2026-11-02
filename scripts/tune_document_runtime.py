#!/usr/bin/env python3
"""Evaluate a bounded document-runtime tuning matrix.

The document benchmark is intentionally the only source of measurements.  This
driver runs each candidate in a fresh process, so model/runtime state and RSS
are not carried from one candidate into the next.  It emits a JSON report and
returns non-zero when no candidate satisfies the resource/quality gates.
"""

from __future__ import annotations

import argparse
import itertools
import json
import math
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any


def _ints(value: str) -> list[int]:
    values = [int(item) for item in value.split(",") if item.strip()]
    if not values or any(item < 0 for item in values):
        raise argparse.ArgumentTypeError("expected one or more non-negative integers")
    return values


def _json_report(stdout: str) -> dict[str, Any]:
    # Cargo wrappers may print build/progress lines before the example's JSON.
    decoder = json.JSONDecoder()
    for offset in range(len(stdout) - 1, -1, -1):
        if stdout[offset] != "{":
            continue
        try:
            value, end = decoder.raw_decode(stdout[offset:])
        except json.JSONDecodeError:
            continue
        if stdout[offset + end :].strip() == "" and isinstance(value, dict):
            return value
    raise ValueError("benchmark output did not end with a JSON object")


def _cold_run(report: dict[str, Any]) -> dict[str, Any]:
    for run in report.get("runs", []):
        if run.get("mode") == "cold":
            return run
    return report.get("runs", [{}])[0]


def _ner_throughput(report: dict[str, Any]) -> tuple[float, float]:
    throughput = _cold_run(report).get("throughput") or {}
    return (
        float(throughput.get("ner_documents_per_second", 0.0)),
        float(throughput.get("ner_tokens_per_second", 0.0)),
    )


def _quality(report: dict[str, Any], require_baseline: bool) -> tuple[float, float, float, bool]:
    quality = report.get("quality") or {}
    retrieval = quality.get("retrieval") or {}
    ner = quality.get("ner") or {}
    comparison = quality.get("baseline_comparison") or {}
    deltas = (
        comparison.get("recall_delta"),
        comparison.get("ndcg_delta"),
        comparison.get("ner_coverage_delta"),
    )
    baseline_complete = not require_baseline or all(value is not None for value in deltas)
    return (
        float(retrieval.get("mean_recall", 0.0)),
        float(retrieval.get("mean_ndcg", 0.0)),
        float(ner.get("coverage", 0.0)),
        bool(comparison.get("within_tolerance", True)) and baseline_complete,
    )


def _stage_corpus(root: Path, manifest: Path, isolated_root: Path) -> Path:
    """Symlink manifest files into an isolated root while keeping relative paths stable."""
    root = root.resolve()
    staged: list[str] = []
    for raw in manifest.read_text(encoding="utf-8").splitlines():
        item = raw.strip()
        if not item or item.startswith("#"):
            continue
        source = (root / item).resolve()
        if not source.is_file():
            fallback = (manifest.parent / item).resolve()
            source = fallback if fallback.is_file() else source
        try:
            relative = source.relative_to(root)
        except ValueError as error:
            raise ValueError(f"manifest entry is outside benchmark root: {item}") from error
        target = isolated_root / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        if not target.exists():
            target.symlink_to(source)
        staged.append(relative.as_posix())
    staged_manifest = isolated_root / "manifest.txt"
    staged_manifest.write_text("\n".join(dict.fromkeys(staged)) + "\n", encoding="utf-8")
    return staged_manifest


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path)
    parser.add_argument(
        "--binary",
        type=Path,
        required=True,
        help="release document_benchmark executable",
    )
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--quality-manifest", type=Path)
    parser.add_argument("--quality-baseline", type=Path)
    parser.add_argument(
        "--throughput-baseline", type=Path, help="prior benchmark JSON for the ingestion-NER throughput gate"
    )
    parser.add_argument(
        "--min-ner-throughput-ratio",
        type=float,
        default=1.0,
        help="minimum candidate/baseline NER token-throughput ratio",
    )
    parser.add_argument("--output", type=Path)
    parser.add_argument("--scan-threads", type=_ints, default=[2])
    parser.add_argument("--embed-threads", type=_ints, default=[2])
    parser.add_argument("--embed-batch-size", type=_ints, default=[16])
    parser.add_argument("--max-concurrent-documents", type=_ints, default=[1])
    parser.add_argument("--onnx-provider", default="cpu")
    parser.add_argument("--rss-ceiling-mb", type=int)
    parser.add_argument("--timeout-seconds", type=float, default=1800.0)
    parser.add_argument("--quality-tolerance", type=float, default=0.01)
    args = parser.parse_args()

    if (
        not math.isfinite(args.timeout_seconds)
        or args.timeout_seconds <= 0
        or args.quality_tolerance < 0
        or not math.isfinite(args.min_ner_throughput_ratio)
        or args.min_ner_throughput_ratio < 0
    ):
        parser.error("timeout must be finite and positive; tolerances and throughput ratio must be non-negative")
    ner_baseline: tuple[float, float] | None = None
    if args.throughput_baseline:
        try:
            baseline_report = json.loads(args.throughput_baseline.read_text(encoding="utf-8"))
            ner_baseline = _ner_throughput(baseline_report)
        except (OSError, json.JSONDecodeError, TypeError, ValueError) as error:
            parser.error(f"invalid --throughput-baseline: {error}")
        if ner_baseline[1] <= 0:
            parser.error("--throughput-baseline has no positive cold ner_tokens_per_second")
    candidates: list[dict[str, int]] = []
    for scan, embed, batch, docs in itertools.product(
        args.scan_threads,
        args.embed_threads,
        args.embed_batch_size,
        args.max_concurrent_documents,
    ):
        if not scan or not embed or not batch or not docs:
            continue
        candidates.append(
            {
                "scan_threads": scan,
                "embed_threads": embed,
                "embed_batch_size": batch,
                "max_concurrent_documents": docs,
            }
        )

    results: list[dict[str, Any]] = []
    for candidate in candidates:
        isolated_root = Path(tempfile.mkdtemp(prefix="basemind-runtime-matrix-"))
        entry: dict[str, Any] = {"configuration": candidate}
        try:
            staged_manifest = _stage_corpus(args.root, args.manifest, isolated_root)
            command = [
                str(args.binary),
                str(isolated_root),
                "--manifest",
                str(staged_manifest),
                "--mode",
                "cold",
                "--onnx-provider",
                args.onnx_provider,
            ]
            for key, flag in (
                ("scan_threads", "--scan-threads"),
                ("embed_threads", "--embed-threads"),
                ("embed_batch_size", "--embed-batch-size"),
                ("max_concurrent_documents", "--max-concurrent-documents"),
            ):
                command.extend((flag, str(candidate[key])))
            if args.quality_manifest:
                command.extend(("--quality-manifest", str(args.quality_manifest)))
            if args.quality_baseline:
                command.extend(
                    (
                        "--quality-baseline",
                        str(args.quality_baseline),
                        "--quality-tolerance",
                        str(args.quality_tolerance),
                    )
                )
            entry["command"] = command
            # shell=False is intentional: every value is passed as one argv element;
            # document contents are never interpolated into a shell command.
            # nosemgrep: python.lang.security.audit.dangerous-subprocess-use-audit
            # nosemgrep: python.lang.security.audit.dangerous-subprocess-use-tainted-env-args
            # nosec B603 -- the benchmark binary and all arguments are explicit argv values.
            completed = subprocess.run(
                command,
                check=False,
                capture_output=True,
                text=True,
                timeout=args.timeout_seconds,
            )
            entry["exit_code"] = completed.returncode
            if completed.returncode != 0:
                entry["failure"] = (completed.stderr or completed.stdout)[-2000:]
                entry["accepted"] = False
                entry["rejection_reasons"] = ["benchmark_failed"]
            else:
                report = _json_report(completed.stdout)
                cold = _cold_run(report)
                quality = _quality(report, args.quality_baseline is not None)
                ner_documents_per_second, ner_tokens_per_second = _ner_throughput(report)
                peak_rss = cold.get("peak_rss_bytes")
                stats = cold.get("stats") or {}
                timed_out = int(stats.get("doc_timeouts", 0)) > 0 or int(stats.get("parse_timeouts", 0)) > 0
                skipped = int(stats.get("docs_deferred", 0)) + int(stats.get("extract_failed", 0))
                entry.update(
                    {
                        "report": report,
                        "cold": cold,
                        "quality": quality[:3],
                        "ner_throughput": {
                            "documents_per_second": ner_documents_per_second,
                            "tokens_per_second": ner_tokens_per_second,
                        },
                        "peak_rss_bytes": peak_rss,
                    }
                )
                reasons: list[str] = []
                if peak_rss is None:
                    reasons.append("rss_unavailable")
                elif args.rss_ceiling_mb is not None and peak_rss > args.rss_ceiling_mb * 1024 * 1024:
                    reasons.append("rss_ceiling")
                if timed_out:
                    reasons.append("timeout")
                skipped += int(stats.get("read_failed", 0))
                skipped += int(stats.get("docs_tombstoned", 0))
                skipped += int(stats.get("skipped_too_large", 0))
                if skipped:
                    reasons.append("skip_or_failure")
                if not quality[3]:
                    reasons.append("quality_regression")
                if ner_baseline is not None and ner_tokens_per_second < ner_baseline[1] * args.min_ner_throughput_ratio:
                    reasons.append("ner_throughput_regression")
                entry["accepted"] = not reasons
                entry["rejection_reasons"] = reasons
        except (OSError, subprocess.TimeoutExpired, ValueError) as error:
            entry["exit_code"] = None
            entry["failure"] = str(error)
            entry["accepted"] = False
            entry["rejection_reasons"] = [
                "timeout" if isinstance(error, subprocess.TimeoutExpired) else "process_error"
            ]
        finally:
            shutil.rmtree(isolated_root, ignore_errors=True)
        results.append(entry)

    accepted = [item for item in results if item.get("accepted")]
    accepted.sort(key=lambda item: -(item.get("cold", {}).get("throughput", {}).get("documents_per_second", 0.0)))
    output = {
        "candidates": len(results),
        "accepted": len(accepted),
        "ner_throughput_baseline": ner_baseline,
        "min_ner_throughput_ratio": args.min_ner_throughput_ratio,
        "best": accepted[0] if accepted else None,
        "results": results,
    }
    encoded = json.dumps(output, indent=2, sort_keys=True)
    if args.output:
        args.output.write_text(encoded + "\n", encoding="utf-8")
    print(encoded)
    return 0 if accepted else 1


if __name__ == "__main__":
    sys.exit(main())
