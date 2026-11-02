#!/usr/bin/env python3
"""Benchmark the standalone redact_text NER path.

The document benchmark measures extraction/indexing. This runner deliberately
invokes ``basemind redact`` so redaction NER latency is reported separately.
It also treats an incomplete NER response as a failed sample: a fast truncated
answer is not a valid performance result for the privacy gateway.
"""

from __future__ import annotations

import argparse
import json
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path


def percentile(samples: list[float], p: float) -> float:
    ordered = sorted(samples)
    rank = max(0, min(len(ordered) - 1, int((p / 100.0) * len(ordered) + 0.999999) - 1))
    return ordered[rank]


def make_text(length: int, run: int) -> str:
    seed = (
        f"Benchmark request {run}: Alice Martin of Acme Conseil reviewed "
        "the contract for 12 rue de Paris, 75001 Paris. "
    )
    return (seed * ((length // len(seed)) + 2))[:length]


def run_one(binary: Path, model: Path, root: Path, profile: str, text: str, timeout: float) -> tuple[float, dict]:
    started = time.perf_counter()
    command = [
        str(binary),
        "--root",
        str(root),
        "redact",
        "--text",
        text,
        "--ner-model-dir",
        str(model),
        "--ner-profile",
        profile,
        "--json",
    ]
    try:
        # shell=False is intentional: all arguments are passed as an argv list,
        # never interpolated into a shell command. The binary/model paths are
        # explicit benchmark inputs, not data from the documents being tested.
        completed = subprocess.run(
            command,
            check=False,
            capture_output=True,
            text=True,
            timeout=timeout,
        )
    except subprocess.TimeoutExpired as exc:
        elapsed_ms = (time.perf_counter() - started) * 1000.0
        raise RuntimeError(f"redact timed out after {elapsed_ms:.1f} ms (limit {timeout:.1f} s)") from exc
    elapsed_ms = (time.perf_counter() - started) * 1000.0
    if completed.returncode != 0:
        raise RuntimeError(
            f"redact failed ({completed.returncode}) after {elapsed_ms:.1f} ms:\n{completed.stderr.strip()}"
        )
    try:
        payload = json.loads(completed.stdout)
    except json.JSONDecodeError as exc:
        raise RuntimeError(f"redact returned invalid JSON: {exc}\n{completed.stdout[:500]}") from exc
    if payload.get("ner_ran") is not True:
        raise RuntimeError("NER did not run; refusing to record a pattern-only sample")
    if payload.get("ner_truncated") is not False:
        raise RuntimeError("NER coverage was truncated; refusing to record an incomplete sample")
    if payload.get("ner_profile", {}).get("name") != profile:
        raise RuntimeError("redact response did not confirm the requested NER profile")
    return elapsed_ms, payload


def assert_deterministic(first: dict, second: dict) -> None:
    fields = (
        "detections",
        "ner_profile",
        "ner_ran",
        "ner_windows",
        "ner_window_chars",
        "ner_truncated",
    )
    for field in fields:
        if first.get(field) != second.get(field):
            raise RuntimeError(f"NER output is not deterministic for field {field!r}")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/release/basemind"))
    parser.add_argument("--model", type=Path, required=True, help="GLiNER2 model directory")
    parser.add_argument("--profile", default="legal-fr@1", choices=("full@1", "legal-fr@1"))
    parser.add_argument("--repetitions", type=int, default=20)
    parser.add_argument("--warmups", type=int, default=1)
    parser.add_argument("--timeout-seconds", type=float, default=120.0)
    parser.add_argument("--baseline", type=Path, help="prior benchmark JSON for p95 comparison")
    parser.add_argument(
        "--max-p95-regression",
        type=float,
        default=0.0,
        help="allowed fractional p95 increase against --baseline (default: 0)",
    )
    parser.add_argument("--output", type=Path, help="write JSON results to this path")
    args = parser.parse_args()
    if args.repetitions < 1 or args.warmups < 0 or args.timeout_seconds <= 0 or args.max_p95_regression < 0:
        parser.error("--repetitions positive, --warmups non-negative, timeout positive, regression non-negative")
    if not args.binary.is_file():
        parser.error(f"binary does not exist: {args.binary}")
    if not args.model.is_dir():
        parser.error(f"model directory does not exist: {args.model}")
    baseline = None
    if args.baseline:
        try:
            baseline = json.loads(args.baseline.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            parser.error(f"cannot read --baseline: {exc}")

    sizes = (200, 2_000, 8_000)
    with tempfile.TemporaryDirectory(prefix="basemind-redact-bench-") as temp_root:
        root = Path(temp_root)
        for run in range(args.warmups):
            run_one(
                args.binary,
                args.model,
                root,
                args.profile,
                make_text(200, -(run + 1)),
                args.timeout_seconds,
            )

        results: dict[str, object] = {
            "binary": str(args.binary),
            "model": str(args.model),
            "profile": args.profile,
            "repetitions": args.repetitions,
            "timing": "cold_cli_process",
            "timeout_seconds": args.timeout_seconds,
            "sizes": {},
        }
        for size in sizes:
            probe_text = make_text(size, -1)
            _, first_probe = run_one(
                args.binary,
                args.model,
                root,
                args.profile,
                probe_text,
                args.timeout_seconds,
            )
            _, second_probe = run_one(
                args.binary,
                args.model,
                root,
                args.profile,
                probe_text,
                args.timeout_seconds,
            )
            assert_deterministic(first_probe, second_probe)
            samples = [
                run_one(
                    args.binary,
                    args.model,
                    root,
                    args.profile,
                    make_text(size, run),
                    args.timeout_seconds,
                )[0]
                for run in range(args.repetitions)
            ]
            summary = {
                "p50_ms": percentile(samples, 50),
                "p95_ms": percentile(samples, 95),
                "mean_ms": statistics.fmean(samples),
                "samples_ms": samples,
            }
            if baseline is not None:
                try:
                    previous = float(baseline["sizes"][str(size)]["p95_ms"])
                except (KeyError, TypeError, ValueError) as exc:
                    raise RuntimeError(f"baseline has no valid p95_ms for {size} chars: {exc}") from exc
                delta = (summary["p95_ms"] - previous) / previous if previous > 0 else 0.0
                summary["baseline_p95_ms"] = previous
                summary["p95_delta_fraction"] = delta
                if delta > args.max_p95_regression:
                    raise RuntimeError(
                        f"{size} chars p95 regressed by {delta:.1%} (allowed {args.max_p95_regression:.1%})"
                    )
            results["sizes"][str(size)] = summary  # type: ignore[index]
            print(f"{size:>5} chars  p50={summary['p50_ms']:.1f} ms  p95={summary['p95_ms']:.1f} ms")

    encoded = json.dumps(results, indent=2) + "\n"
    if args.output:
        args.output.write_text(encoded, encoding="utf-8")
    else:
        print(encoded, end="")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except RuntimeError as exc:
        print(f"error: {exc}", file=sys.stderr)
        raise SystemExit(1)
