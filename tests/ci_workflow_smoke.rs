//! Canary on the CI cost controls in `ci.yaml`.
//!
//! These exist because the `full` leg is the most expensive job in this repo and every regression
//! here is silent: the job still goes green, just four hours later. The two that bit on
//! 2026-10-09 are recorded below.
//!
//! Structural assertions over the workflow text (no YAML dependency), matching the style of
//! `publish_workflow_smoke.rs`.

use std::path::PathBuf;

fn workflow() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".github/workflows/ci.yaml");
    std::fs::read_to_string(&path)
        .expect("read .github/workflows/ci.yaml")
        .replace("\r\n", "\n")
}

/// The body of a top-level job: from its 2-space-indented key to the next one. Step entries live
/// at 6 spaces under `steps:`, so only 2-space non-space lines are sibling job headers.
fn job_block<'a>(workflow: &'a str, job: &str) -> &'a str {
    let marker = format!("\n  {job}:\n");
    let start = workflow
        .find(&marker)
        .unwrap_or_else(|| panic!("job `{job}` not found in ci.yaml"))
        + 1;
    let rest = &workflow[start..];
    let after_header = rest.find('\n').map_or(rest.len(), |i| i + 1);
    let mut offset = after_header;
    for line in rest[after_header..].split_inclusive('\n') {
        let bytes = line.as_bytes();
        let is_job_header =
            bytes.len() >= 3 && bytes[0] == b' ' && bytes[1] == b' ' && bytes[2] != b' ' && bytes[2] != b'#';
        if is_job_header {
            return &rest[..offset];
        }
        offset += line.len();
    }
    rest
}

/// The `test` matrix job, which every platform leg runs through.
fn test_job(workflow: &str) -> &str {
    job_block(workflow, "test")
}

/// A cache that is only written on success turns one slow run into a loop.
///
/// `Swatinem/rust-cache` does not save when a job fails or is cancelled unless `cache-on-failure`
/// is set, and `cancel-in-progress` cancels superseded PR runs. So a leg that exceeds its timeout
/// leaves nothing behind, and its retry — cold, and therefore slower than the run that just timed
/// out — is the one most likely to time out too. Observed on 2026-10-09: `ubuntu-latest / full`
/// cancelled at exactly its 240m budget, and the re-run started cold from the same commit that had
/// gone green in 73 minutes warm.
#[test]
fn rust_cache_saves_even_when_the_job_does_not_pass() {
    let workflow = workflow();
    let test = test_job(&workflow);
    assert!(
        test.contains("cache-on-failure: true"),
        "rust-cache must set `cache-on-failure: true`. Without it a timed-out leg leaves no cache, \
         so every retry is cold and slower, and the leg can never recover on its own."
    );
}

/// The release profile must not be rebuilt on every pull request.
///
/// Measured on the 73-minute green run of 2026-10-09: `cargo build --release` was 23m04s, 32% of
/// the leg. A release profile is a different fingerprint from the debug and test builds above it, so
/// it recompiles the whole dependency graph rather than reusing anything. `publish.yaml` builds
/// `--release` for all six release targets, so dropping it from PRs loses no coverage of the
/// artifact — but it must still run somewhere, or a release-only compile break is first discovered
/// at publish time, which is after the tag.
#[test]
fn the_release_build_is_not_paid_for_on_pull_requests() {
    let workflow = workflow();
    let test = test_job(&workflow);
    assert!(
        test.contains("cargo build --release"),
        "the release build must still exist somewhere in the matrix job"
    );
    let release_block = test
        .split("- name: cargo build --release")
        .nth(1)
        .expect("a `cargo build --release` step exists");
    assert!(
        release_block.contains("github.event_name == 'push'"),
        "`cargo build --release` must be gated to pushes, not run on every pull request: it is 32% \
         of the leg and recomputes every dependency under the release profile."
    );
}

/// mold and sccache must stay out until they are shown to work.
///
/// Both were added to this workflow on 2026-10-10 and both were removed again the same day.
///
/// mold installed fine (2.30.0 from apt) and exported `-C link-arg=-fuse-ld=mold`, and the `full`
/// leg then failed in `ort-sys` with `could not find native static library 'onnxruntime'`. sccache
/// was worse in a quieter way: `mozilla-actions/sccache-action@v0.0.9` does not accept
/// `rustc-wrapper` or `cache-size`, warned, ignored both, and left `RUSTC_WRAPPER` unset — so
/// sccache served zero compilations while the workflow read as though it were configured.
///
/// A step that installs a tool is not the same as a tool in use, and a step that reads as configured
/// is not evidence. Whoever reinstates either has to show the compiler env actually reaches rustc.
#[test]
fn no_unverified_linker_or_cache_tool_is_reintroduced_silently() {
    // Comments are stripped first: the note explaining why these were removed necessarily names
    // them, and a test that matched prose would fail on its own documentation.
    let live = workflow()
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");

    for banned in ["fuse-ld=mold", "sccache-action"] {
        assert!(
            !live.contains(banned),
            "`{banned}` is not in use in CI. mold broke the `full` leg's `ort-sys` link, and \
             sccache was installed without `RUSTC_WRAPPER` ever being set, so it compiled nothing. \
             Reinstate only with evidence: a passing `full` leg and the compiler env in the log."
        );
    }
}
