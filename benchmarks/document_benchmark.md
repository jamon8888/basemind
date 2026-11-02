# PDF/Office indexing benchmark

Generate deterministic format-coverage fixtures inside the benchmark root:

```sh
python3 benchmarks/generate_document_fixtures.py path/to/workspace/fixtures
```

Create a private manifest with paths relative to the workspace root, then run:

```sh
cargo run --release --features documents --example document_benchmark -- \
  path/to/workspace \
  --manifest private-manifest.txt \
  --synthetic-manifest fixtures/synthetic-manifest.txt \
  --quality-manifest benchmarks/document_quality.example.jsonl \
  --mode all
```

The harness also auto-discovers `fixtures/synthetic-manifest.txt` (or a root
`synthetic-manifest.txt`) when `--synthetic-manifest` is omitted.

`all` executes cold, warm, and a one-file incremental pass. Each run reports
stage timings, first persisted-searchable-result time, processed bytes/tokens/
chunks throughput, RSS, configuration, and skip/failure counters. The throughput
section also reports `ner_documents_per_second` and `ner_tokens_per_second`,
derived only from the ingestion NER stage timing; these are intentionally
separate from the `redact_text` NER path. The optional quality manifest reports
embedding recall/nDCG and NER category/text coverage.
When supplied, `--quality-baseline` adds recall, nDCG, and NER coverage deltas
against a prior JSON report (either the full benchmark output or its `quality`
object). The default allowed regression is `0.01` (one percentage point); set
`--quality-tolerance` to change that fraction. A regression beyond the limit
fails the benchmark command.
Fixtures are intentionally small; use the private manifest for throughput and
memory thresholds.

To sweep runtime settings without editing the config file, pass
`--scan-threads`, `--embed-threads`, `--embed-batch-size`,
`--max-concurrent-documents`, and `--onnx-provider` (`cpu`, `auto`, `coreml`,
`cuda`, or `tensorrt`). Every selected value is echoed in the JSON
`configuration` object, so matrix results remain comparable and reproducible.

For a proportional update benchmark, edit one manifest file between separate
`--mode warm` and `--mode incremental` invocations, or select it explicitly
with `--incremental-file path/to/file.docx`.

The generated `synthetic-incremental.txt` is selected automatically and is
updated in `all`/`incremental` runs. Use `--mutate-incremental` to opt into the
same deterministic append for a selected text fixture.

## Runtime tuning matrix

Use `scripts/tune_document_runtime.py` to compare bounded runtime settings in
fresh processes. The driver runs the cold benchmark once per combination of
scanner threads, embedding threads, embedding batch size, and document
concurrency, using a temporary isolated store for every candidate so prior
indexes cannot turn a later run into a warm scan. A candidate is rejected when
it times out, reports any read/extraction/tombstone/oversized-file skip, exceeds
`--rss-ceiling-mb`, or fails the supplied quality baseline. The fastest
remaining candidate is reported as `best`; no candidate is silently selected
when every combination violates a gate.

```sh
python3 scripts/tune_document_runtime.py /private/docs \
  --binary target/release/examples/document_benchmark \
  --manifest /private/docs-manifest.txt \
  --quality-manifest benchmarks/document_quality.example.jsonl \
  --quality-baseline /private/baseline.json \
  --scan-threads 2,4 --embed-threads 2,4 \
  --embed-batch-size 16,32 --max-concurrent-documents 1,2 \
  --rss-ceiling-mb 8192 --output /private/runtime-matrix.json
```

The matrix is evidence-only: CPU remains the reference provider, and an
alternate provider must be passed explicitly and measured on the target corpus
before it is adopted as a deployment default.
