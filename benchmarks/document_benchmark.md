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
chunks throughput, RSS, configuration, and skip/failure counters. The optional
quality manifest reports embedding recall/nDCG and NER category/text coverage.
When supplied, `--quality-baseline` adds recall, nDCG, and NER coverage deltas
against a prior JSON report (either the full benchmark output or its `quality`
object).
Fixtures are intentionally small; use the private manifest for throughput and
memory thresholds.

For a proportional update benchmark, edit one manifest file between separate
`--mode warm` and `--mode incremental` invocations, or select it explicitly
with `--incremental-file path/to/file.docx`.

The generated `synthetic-incremental.txt` is selected automatically and is
updated in `all`/`incremental` runs. Use `--mutate-incremental` to opt into the
same deterministic append for a selected text fixture.
