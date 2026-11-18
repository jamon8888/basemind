# PR Status: OCR languages parameter

## Branch
- `jamon8888/issue-OCR` is 10 commits ahead of `main`
- No open PR associated with this branch

## Changes Summary
The PR adds per-call OCR languages parameter support for `redact_text` and `basemind redact`:

### 1. OCR Languages Parameter
- `RedactTextParams` now has optional `ocr_languages: Option<Vec<String>>` field
- `resolve_ocr_languages()` function: uses per-call override if provided/non-empty, otherwise falls back to configured default
- CLI `basemind redact --file` gets `--ocr-languages fra,eng` argument
- MCP server `redact_text` tool respects the override

### 2. Bundled Tessdata Configuration
- `src/extract/ocr_assets.rs` adds:
  - `bundled_tessdata_for(languages)` → `Result<Option<PathBuf>>` — returns bundled tessdata path if all requested languages are covered by built-in packs (eng, deu, fra, spa, ita, nld, pol), otherwise `None`
  - `xberg_ocr_config(languages, tessdata_path)` — creates OCR config with tessdata_path
- Extraction config uses `use_cache: false` to avoid cache-related issues

### 3. Test Fixtures
- `tests/fixtures/ocr/scan-fr.pdf` — French PDF (3 lines, Arial 40px, 1300x260)
- `tests/fixtures/ocr/scan-fr.png` — Same content at 2400x480 → 1600x320 grayscale (66 KB)
- Both use 7 default Tesseract languages + `fra` + `eng` to read all 3 lines

### 4. New Integration Test
- `tests/redact_file_extraction.rs` — Tests `redact_text` on file_path through CLI
- Verifies no clear-text copy on disk
- Tests OCR with bundled tessdata

### 5. Test Updates
- `src/mcp/redact_file_tests.rs` — Updated test calls to pass `None` for `ocr_languages`
- All `run_redact()` calls now take `ocr_languages: Option<Vec<String>>` parameter

### 6. CLI Changes
- `src/cli/redact.rs` adds `ocr_languages: Vec<String>` argument
- Empty value becomes `None`, non-empty becomes override

### 7. CHANGELOG
- Added "Unreleased" section with the OCR languages parameter feature

## Verification
- `cargo check --lib --features ""` — ✅ Compiles successfully
- `cargo clippy --lib --features ""` — ✅ No warnings

## CI Status
- Full CI matrix would test across multiple OS/feature combinations
- Individual job durations range from ~180s (test) to ~240s (full feature)
- Cache management is critical (ENOSPC issues on Linux full leg)

## Next Steps (pending user clarification)
- [ ] Create/open PR for review/merge
- [ ] Run specific test subsets to validate changes
- [ ] Address any remaining CI configuration issues
- [ ] Other: ________