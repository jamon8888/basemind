# End-to-end benchmark verification

`document_benchmark` emits measurements, while
`scripts/verify_document_benchmark.py` turns the integration contract into a
repeatable release gate. It checks stage timings, first-searchable-result
telemetry, throughput, RSS, resource configuration, and skip/failure counters'
presence for cold, warm, and incremental runs.

```sh
cargo run --release --features documents --example document_benchmark -- \
  /path/to/workspace --mode all --manifest private-manifest.txt \
  --synthetic-manifest fixtures/synthetic-manifest.txt > benchmark.json

python3 scripts/verify_document_benchmark.py benchmark.json \
  --require-modes cold,warm,incremental \
  --warm-fraction 0.10 \
  --require-quality
```

The warm target and RSS ceiling are explicit command-line gates rather than
hard-coded assumptions. Use a private-folder-specific ceiling when the target
machine has a known memory budget. `--require-quality` requires the benchmark's
retrieval and NER quality object, including zero invalid spans; use the existing
`--quality-baseline` and `--quality-tolerance` options when comparing releases.
