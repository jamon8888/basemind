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
import subprocess
import sys
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


def _quality(report: dict[str, Any]) -> tuple[float, float, float, bool]:
    quality = report.get("quality") or {}
    retrieval = quality.get("retrieval") or {}
    ner = quality.get("ner") or {}
    comparison = quality.get("baseline_comparison") or {}
    return (
        float(retrieval.get("mean_recall", 0.0)),
        float(retrieval.get("mean_ndcg", 0.0)),
        float(ner.get("coverage", 0.0)),
        bool(comparison.get("within_tolerance", True)),
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path)
    parser.add_argument(
        "--binary", type=Path, required=True, help="release document_benchmark executable"
    )
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--quality-manifest", type=Path)
    parser.add_argument("--quality-baseline", type=Path)
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

    if args.timeout_seconds <= 0 or args.quality_tolerance < 0:
        parser.error(
            "timeout and quality tolerance must be non-negative, with timeout > 0"
        )
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
        command = [
            str(args.binary),
            str(args.root),
            "--manifest",
            str(args.manifest),
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

        entry: dict[str, Any] = {"configuration": candidate, "command": command}
        try:
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
            else:
                report = _json_report(completed.stdout)
                cold = _cold_run(report)
                quality = _quality(report)
                peak_rss = cold.get("peak_rss_delta_bytes") or cold.get("peak_rss_bytes") or 0
                stats = cold.get("stats") or {}
                timed_out = int(stats.get("doc_timeouts", 0)) > 0 or int(
                    stats.get("parse_timeouts", 0)
                ) > 0
                skipped = int(stats.get("docs_deferred", 0)) + int(stats.get("extract_failed", 0))
                entry.update(
                    {
                        "report": report,
                        "cold": cold,
                        "quality": quality[:3],
                        "peak_rss_bytes": peak_rss,
                    }
                )
                reasons: list[str] = []
                if args.rss_ceiling_mb is not None and peak_rss > args.rss_ceiling_mb * 1024 * 1024:
                    reasons.append("rss_ceiling")
                if timed_out:
                    reasons.append("timeout")
                if skipped:
                    reasons.append("skip_or_failure")
                if not quality[3]:
                    reasons.append("quality_regression")
                entry["accepted"] = not reasons
                entry["rejection_reasons"] = reasons
        except (OSError, subprocess.TimeoutExpired, ValueError) as error:
            entry["exit_code"] = None
            entry["failure"] = str(error)
        results.append(entry)

    accepted = [item for item in results if item.get("accepted")]
    accepted.sort(
        key=lambda item: -(
            item.get("cold", {}).get("throughput", {}).get("documents_per_second", 0.0)
        )
    )
    output = {
        "candidates": len(results),
        "accepted": len(accepted),
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
