//! Canary locking the `publish.yaml` ordering invariants so a later refactor cannot
//! silently let a registry publish run before the GitHub release is finalized, or let a
//! partial draft masquerade as a complete release. Post-mortem of the v0.22.1 release
//! window: the plugin/npm `latest` pointer and the crates.io push must only advance once
//! every platform asset exists and the release has been promoted — otherwise a downstream
//! clean install resolves a version whose binaries are not yet downloadable.
//!
//! These are structural assertions over the workflow text (no YAML dependency): they slice
//! a job's block and check the `needs` / `if` gating and the required-asset set within it.

use std::path::PathBuf;

fn workflow() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".github/workflows/publish.yaml");
    std::fs::read_to_string(&path)
        .expect("read .github/workflows/publish.yaml")
        .replace("\r\n", "\n")
}

/// The body of a top-level job: from its 2-space-indented `name:` header to the next
/// 2-space-indented job header. Steps live at deeper indentation, so the only 2-space
/// non-space lines are sibling job headers.
fn job_block<'a>(workflow: &'a str, job: &str) -> &'a str {
    let marker = format!("\n  {job}:\n");
    let start = workflow
        .find(&marker)
        .unwrap_or_else(|| panic!("job `{job}` not found in publish.yaml"))
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

fn assert_gated_on_finalize(workflow: &str, job: &str) {
    let block = job_block(workflow, job);
    assert!(
        block.contains("finalize_release"),
        "job `{job}` must list finalize_release in `needs` so it cannot publish before the release finalizes",
    );
    assert!(
        block.contains("needs.finalize_release.result == 'success'"),
        "job `{job}` must gate its `if:` on finalize_release success",
    );
}

/// Every irreversible registry publish must wait for a finalized GitHub release.
#[test]
fn registry_publishes_wait_for_a_finalized_release() {
    let workflow = workflow();
    for job in [
        "publish_npm",
        "publish_opencode",
        "publish_pypi",
        "publish_pypi_hermes",
        "publish_crates",
    ] {
        assert_gated_on_finalize(&workflow, job);
    }
}

/// The `github_only` dispatch input must skip every registry, and only the registries.
///
/// A GitHub-only release is still a real release: the binaries are built, the checksums are
/// computed, and the draft is promoted with the full asset set. What it does not do is move a
/// version pointer on npm, PyPI, crates.io or the homebrew tap — those are irreversible and are
/// not implied by "put the binaries on GitHub".
///
/// Both directions are asserted. A registry job that lost its gate would publish a version the
/// operator explicitly declined to publish, which is the worse failure and the harder to notice.
/// A release-chain job that gained the gate would produce a release with no binaries.
#[test]
fn github_only_skips_every_registry_and_nothing_else() {
    let workflow = workflow();

    for job in [
        "publish_npm",
        "publish_opencode",
        "publish_pypi",
        "publish_pypi_hermes",
        "publish_crates",
        "publish_homebrew",
    ] {
        let block = job_block(&workflow, job);
        assert!(
            block.contains("needs.meta.outputs.github_only != 'true'"),
            "job `{job}` publishes irreversibly and must be skipped under github_only; without this \
             gate a GitHub-only release still moves a version pointer"
        );
    }

    // The chain that produces the release itself must run regardless.
    for job in [
        "create_release",
        "build-binaries",
        "build-linux-binaries",
        "build-linux-noavx2",
        "checksums",
        "finalize_release",
    ] {
        let block = job_block(&workflow, job);
        assert!(
            !block.contains("github_only"),
            "job `{job}` builds or finalizes the GitHub release, which github_only still does; \
             gating it would produce a release with no binaries"
        );
    }

    // And the flag must be a dispatch-only input, not a tag-push behaviour change: a tag has
    // always meant "publish everywhere", and silently redefining that would make this tag do
    // less than every tag before it. Asserted on the resolution step itself rather than on the
    // input's mere presence, because a `github_only:` input that the resolution ignores — or
    // that any trigger can set — is exactly the regression this is here to catch.
    let meta = job_block(&workflow, "meta");
    assert!(
        meta.contains("skip_registries"),
        "meta must resolve the github_only flag"
    );
    let resolution = meta
        .split("id: skip_registries")
        .nth(1)
        .expect("the skip_registries step must exist");
    assert!(
        // Shell string equality inside the workflow: `= "workflow_dispatch"`, not `==`, and the
        // expression is interpolated as `${{ github.event_name }}` so there is a `}}` between
        // `event_name` and the `=`.
        resolution.contains("\" = \"workflow_dispatch\"")
            || resolution.contains("\" = 'workflow_dispatch'")
            || resolution.contains("github.event_name == 'workflow_dispatch'"),
        "github_only must be honoured only on a manual dispatch; a tag push must keep publishing \
         every registry, as every tag before this one did"
    );
    assert!(
        workflow.contains("github_only:"),
        "the dispatch must expose a `github_only` input"
    );
}

/// Binary releases with Git-pinned dependencies cannot be reconstructed by crates.io, so
/// the source-crate publish must be skipped without blocking the platform artifacts.
#[test]
fn crates_publish_is_skipped_for_git_dependencies() {
    let workflow = workflow();
    let meta = job_block(&workflow, "meta");
    let publish = job_block(&workflow, "publish_crates");
    assert!(
        meta.contains("crates_publishable=false") && meta.contains("git[[:space:]]*="),
        "meta must detect Git dependencies in the root Cargo manifest",
    );
    assert!(
        publish.contains("needs.meta.outputs.crates_publishable == 'true'"),
        "publish_crates must skip source publication when Git dependencies are present",
    );
}

/// Promotion (which flips the release public and lets `latest` move) must require the full
/// platform-asset set plus checksums, not a partial draft.
#[test]
fn finalize_requires_the_full_asset_set() {
    let workflow = workflow();
    let block = job_block(&workflow, "finalize_release");
    for asset in [
        "basemind-x86_64-unknown-linux-gnu.tar.gz",
        "basemind-x86_64-unknown-linux-gnu-noavx2.tar.gz",
        "basemind-aarch64-unknown-linux-gnu.tar.gz",
        "basemind-aarch64-apple-darwin.tar.gz",
        "basemind-x86_64-apple-darwin.tar.gz",
        "basemind-x86_64-pc-windows-msvc.zip",
    ] {
        assert!(
            block.contains(asset),
            "finalize_release must require {asset} before promoting"
        );
    }
    assert!(
        block.contains("_checksums.txt"),
        "finalize_release must require the checksums file before promoting",
    );
}

/// The "already published?" gate must count missing required assets, so a partial draft
/// reports incomplete and the build matrix reruns to heal it — the previous "any asset
/// present" check skipped the builders and a partial release could never complete.
#[test]
fn complete_release_detection_counts_the_full_set() {
    let workflow = workflow();
    let block = job_block(&workflow, "meta");
    assert!(
        block.contains("required=(") && block.contains("missing"),
        "meta must gate release_assets_exist on the full required asset set, not on any single asset",
    );
}

/// A publish run is never cancelled mid-flight: cargo publish and release promotion are
/// irreversible, so `cancel-in-progress` must be false.
#[test]
fn publish_is_never_cancelled_mid_flight() {
    assert!(
        workflow().contains("cancel-in-progress: false"),
        "publish concurrency must set cancel-in-progress: false",
    );
}
