# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelok.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

<!-- Keep a Changelog repeats Added/Changed/Fixed headings per version. -->
<!-- markdownlint-disable MD024 -->

## [0.39.0] - 2026-11-20

> **Minor release.** This release uses persisted-schema version `39`; the first
> scan after upgrading may pay one full re-index (durable, not destructive — see
> `src/version.rs`).

### Fixed

- macOS CI: drop debug symbols on `full` leg to prevent ENOSPC disk exhaustion (`debug=0`)
- macOS CI: add `continue-on-error` for slow Intel macOS x86_64 leg (`intel-macos`)
- macOS CI: increase `BASEMIND_COMMS_HANDSHAKE_TIMEOUT_SECS` to 120s for daemon IPC headroom
- macOS CI: fetch ONNX Runtime dylib from immutable PyPI wheel instead of removed GitHub asset
- macOS CI: raise memory-ceiling timeout to 60min for full scanner builds
- macOS CI: fix cgroup harness self-check to use Python instead of `tail /dev/zero`

### Changed

- Release: version surfaces are bumped by `scripts/release-bump.sh`, which now
  regenerates the plugin bundle from `plugin/`, synchronizes the public-skills and
  legacy compatibility exports, and validates every surface before it finishes.
- CI: `continue-on-error` moved to the job level so the workflow parses; the
  `poly fmt`/`poly lint` checks pass, and `docs/specs` is no longer read as an ADR.
- Repo: deleted 164 source files that no module declares, and the `index_read`
  bench that had not compiled since the read path moved to `IndexDb`.
- Bump version to 0.39.0

## [0.38.0] - 2026-11-14

> **Minor release.** This release uses persisted-schema version `38`; the first
> scan after upgrading may pay one full re-index (durable, not destructive — see
> `src/version.rs`).

### Added

- `redact_text` now offers the versioned `legal-fr@2` NER profile: named private
  places and document dates are covered, broad legal geography remains readable,
  and common French legal references are not mistaken for phone numbers (#97).
- `BASEMIND_FILE_EXTRACTION_TIMEOUT_SECS` (default 120, at most 3600) sets the shared
  budget for extracting one file in `redact_text`/`redact --file`, OCR retry included, so a
  caller with a longer deadline of its own is not cut off first.
- New `hacienda-legal` focus release for legal client folders: a `--features hacienda-legal` build (documents + memory + `ner-candle` + `graphrag`, no comms/shells/crawl/code-intel) shipped as `basemind-hacienda-legal-<triple>` release assets, with a drop-in client-folder preset at `examples/basemind-hacienda-legal.toml` (ingestion-time `token-replace` redaction, legal NER labels, multilingual retrieval lanes, GraphRAG) and a new "Legal workspaces" docs page covering the redact → `vault encrypt` → caller-persisted blob → `vault decrypt`/`find` rehydration flow.
- The PDF/Office benchmark accepts runtime concurrency and ONNX-provider overrides for measured tuning sweeps.
- Document search supports opaque continuation cursors, with durable generation invalidation when the Lance index is rebuilt.

### Changed

- Document-feature builds now bundle the configured default Tesseract language packs, including
  French, so first OCR use is ready offline instead of downloading language data.
- Add `[resources] keep_models_warm` so long-lived daemons can retain ONNX embedding and NER engines
  between passes, avoiding cold model reloads at the cost of resident memory.
- Document scans reuse extracted chunks and NER results when only the embedding preset changes;
  cached chunks are re-embedded in place instead of re-running format extraction.

### Fixed

- `basemind serve` on builds without `comms` (including `hacienda-legal`) no longer panics at
  startup with "there is no reactor running": the Tokio runtime is now entered before the server
  and its background watchers are built.
- `basemind serve` on builds without `comms` (including `hacienda-legal`) now catches up at startup
  on an already-populated index: files and documents added, edited or removed while no serve held
  the index are rescanned incrementally, keyword lane first, then an embedding pass only when
  something changed, so an up-to-date workspace does not load the embedding model at boot.
  Previously the boot scan ran for an empty index only and the live watcher only sees later
  changes, so such files stayed unsearchable. The daemon-hosted server of `comms` builds does not
  run this catch-up yet.
- Under the `legal-fr` profile, a span of years such as `2024-2025` (1900-2099, second year one
  to ten years after the first) is no longer redacted as a phone number, even without a
  reference cue nearby; a phone cue next to it still wins, and so does a dialing prefix or a
  trailing digit group (`(21) 2012-2019`, `+55 21 2012-2019`), since some countries write
  eight-digit numbers as two groups of four.
- The OCR retry after dictionary filtering now also runs on scanned PDFs: xberg keeps the
  "Tesseract removed N OCR line(s)" warning but not the dictionary ratio on a PDF, so a
  whole-document OCR result without the ratio retries on the warning alone (#115).
- `tests/hacienda_legal_smoke.rs` serves the canonical workspace path, as the CLI scan does;
  on macOS the raw tempdir path (a `/var` symlink) found no document hits.
- OCR no longer loses real text when Tesseract's dictionary signal is unavailable: document OCR
  retries with xberg's plain-text path only when the normal hOCS pass reports dictionary-filtered
  lines, preserving structured table OCR for healthy documents.
- The document benchmark verifier now validates every emitted skip/defer counter and runtime
  setting, plus non-negative timing, throughput, RSS, and first-result telemetry; incomplete or
  malformed reports can no longer pass release gates.
- Benchmark runs now retain a private-path `skip_reasons` list for every bounded-out or failed file,
  making document coverage decisions diagnosable alongside aggregate counters.
- Transparent raster images are composited over white before OCR, preventing alpha
  channels from becoming black pages in xberg's RGB conversion path.
- Document cache fingerprints now include the configured ONNX provider, preventing
  embeddings, NER, layout, or OCR outputs from being reused across runtimes with
  different execution behavior.
- Document metadata now preserves xberg's exact count of OCR lines removed by its
  dictionary filter, making partial OCR loss visible to indexing and diagnostics.
- Large linear-chunked documents now retain explicit Roman and dotted-Arabic section
  breadcrumbs (including their citation numbers) for lexical and dense retrieval.
- File-redaction timeout diagnostics now report cumulative shared-budget usage and the current
  extraction attempt duration.

## [0.36.0] - 2026-10-31
