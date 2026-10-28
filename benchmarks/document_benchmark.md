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

`all` executes cold, warm, and a one-file incremental pass. Each run reports
stage timings, first persisted-searchable-result time, processed bytes/tokens/
chunks throughput, RSS, configuration, and skip/failure counters. The optional
quality manifest reports embedding recall/nDCG and NER category/text coverage.
Fixtures are intentionally small; use the private manifest for throughput and
memory thresholds.
