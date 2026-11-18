# PR: OCR languages parameter for redact_text

## Summary
Add per-call OCR languages parameter support for `redact_text` MCP tool and `basemind redact` CLI, allowing users to override the configured `[documents.ocr] languages` default when redacting images or scanned PDFs.

## Motivation
Previously, `redact_text` always used the configured default OCR languages from `[documents.ocr] languages`. Users needed the ability to override this per-call (e.g., `fra,eng` for a French PDF), especially when working with scanned documents in specific languages.

## Changes

### 1. OCR Languages Parameter (`RedactTextParams`)
- Added optional `ocr_languages: Option<Vec<String>>` field to `RedactTextParams` struct
- Added `resolve_ocr_languages()` function that uses per-call override if provided/non-empty, otherwise falls back to configured default

### 2. MCP Server `redact_text` tool
- `run_redact()` now accepts `ocr_languages: &[String]` parameter
- Calls `resolve_ocr_languages()` to determine which languages to use
- Passes resolved languages to `run_redact()` which uses them for `file_extraction_config()`

### 3. CLI `basemind redact --file`
- Added `--ocr-languages LANGS` argument (comma-separated, ISO 639-3 codes like `fra`, `eng`)
- When set, overrides `[documents.ocr] languages` config
- When not set, falls back to configured default

### 4. Bundled Tessdata Configuration
- `src/extract/ocr_assets.rs`:
  - `bundled_tessdata_for(languages)` → returns `Some(PathBuf)` if all requested languages are covered by built-in packs (eng, deu, fra, spa, ita, nld, pol), otherwise `None`
  - `xberg_ocr_config(languages, tessdata_path)` — creates OCR config with tessdata_path
- Extraction config uses `use_cache: false` to avoid cache-related issues (xberg 1.3.9 leaves OCR result cache on for scanned PDFs)

### 5. Test Updates
- All `run_redact()` calls now take `ocr_languages: Option<Vec<String>>` parameter
- Tests pass `None` for the new parameter (backward compatible)
- New test fixtures: `tests/fixtures/ocr/scan-fr.pdf`, `tests/fixtures/ocr/scan-fr.png`
- New integration test: `tests/redact_file_extraction.rs`

### 6. CHANGELOG
- Added "Unreleased" section documenting the OCR languages parameter feature

## Files Changed
- `src/extract/ocr_assets.rs` — New bundled tessdata functions
- `src/mcp/tools_redact.rs` — Core logic: `resolve_ocr_languages()`, `file_extraction_config()`, updated `run_redact()`
- `src/mcp/redact_file_tests.rs` — Updated test calls
- `src/cli/redact.rs` — CLI `--ocr-languages` argument
- `tests/fixtures/ocr/scan-fr.pdf` — French PDF fixture (Bin 28697 bytes)
- `tests/fixtures/ocr/scan-fr.png` — French PNG fixture (Bin 66095 bytes)
- `tests/redact_file_extraction.rs` — New integration test (241 lines)
- `CHANGELOG.md` — Added unreleased section

## Verification
- `cargo check --lib --features ""` — ✅ Compiles successfully
- `cargo clippy --lib --features ""` — ✅ No warnings

## Review checklist
- [x] Code compiles without errors
- [x] Clippy passes with no warnings
- [x] Backward compatible (empty `ocr_languages` → falls back to config)
- [x] Bundled tessdata used when languages are covered by built-in packs
- [x] `use_cache: false` set for extraction config
- [x] Test fixtures added and referenced
- [x] Integration test covers file_path OCR + redaction flow