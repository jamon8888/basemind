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
extraction timings.
