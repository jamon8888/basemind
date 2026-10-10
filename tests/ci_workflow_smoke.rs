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

/// mold is a Linux-only linker, so both halves of using it must be Linux-only.
///
/// The flag and the binary are set in the same step on purpose. If the apt package were ever
/// unavailable and the flag were still exported, every link on the runner would fail on a linker
/// that does not exist — a red CI for a missing optional tool. Falling back to the default linker
/// costs a few minutes and breaks nothing.
#[test]
fn the_mold_linker_is_installed_and_selected_on_linux_only() {
    let workflow = workflow();
    let test = test_job(&workflow);

    let mold = test
        .split("- name: Install mold linker (Linux)")
        .nth(1)
        .expect("a mold install step exists");
    assert!(
        mold.contains("runner.os == 'Linux'"),
        "mold is not available on the macOS or Windows legs; installing it unconditionally is a \
         wasted step at best"
    );
    assert!(
        mold.contains("fuse-ld=mold"),
        "the mold step must export the linker flag"
    );

    // The flag is written to GITHUB_ENV from inside the step that installs the binary, and behind
    // the install succeeding. Assert the guard, not just the presence of the flag.
    assert!(
        mold.contains("GITHUB_ENV"),
        "the linker flag must be exported through GITHUB_ENV so later steps see it"
    );
    assert!(
        mold.contains("apt-get install") && mold.contains("if sudo apt-get install"),
        "the flag must only be exported when the install actually succeeded, so a runner without \
         the package falls back to the default linker instead of failing to link"
    );

    // No global RUSTFLAGS: the Windows leg already routes zlib's import-library path through a
    // target-scoped variable, and a job-level RUSTFLAGS would collide with it.
    assert!(
        !workflow.contains("\n    RUSTFLAGS:"),
        "do not set a global RUSTFLAGS; the Windows zlib step uses CARGO_TARGET_*_RUSTFLAGS and a \
         global value would clobber it"
    );
}
