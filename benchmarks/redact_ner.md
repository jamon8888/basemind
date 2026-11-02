# `redact_text` NER benchmark

`document_benchmark` measures extraction and indexing. This benchmark measures
the separate privacy-gateway path by invoking the shipped `basemind redact`
command for unique 200, 2,000, and 8,000 character inputs.

Build the release binary with the same document/NER features used in production,
then run:

```sh
cargo build --release --features documents
python3 scripts/bench_redact_ner.py \
  --binary target/release/basemind \
  --model /path/to/gliner2 \
  --profile legal-fr@1 \
  --repetitions 20 \
  --output redact-ner.json
```

The first warm-up call is excluded from the reported samples. Each sample is a
fresh CLI process, so the result is intentionally labeled
`timing: "cold_cli_process"` and includes process/model startup; it must not be
presented as warm in-process inference latency. Every measured sample must
confirm `ner_ran: true`, `ner_truncated: false`, and the requested profile in
the response. The command exits non-zero instead of recording a pattern-only,
partially covered, or timed-out result. The JSON includes p50, p95, mean, and
raw samples for each size, making it suitable for comparing complete gateway
request cost and provider/runtime configurations without mixing in document
extraction timings. Before timing each size, the runner sends the same text
twice and compares detections plus coverage metadata; a fixed provider/config
that produces different spans fails the benchmark. Each invocation uses an
isolated temporary `--root`, so concurrent benchmark runs cannot append to the
same workspace telemetry file.

To enforce a latency regression gate against a prior run:

```sh
python3 scripts/bench_redact_ner.py --model /path/to/gliner2 \
  --baseline redact-ner-baseline.json --max-p95-regression 0
```

The baseline must be a previous JSON result from this runner. The command adds
the prior p95 and fractional delta to each size and exits non-zero when any p95
exceeds the allowed regression. Precision/recall still requires a labeled
gateway corpus; the document benchmark's NER quality manifest is the reference
format for that separate quality gate.
