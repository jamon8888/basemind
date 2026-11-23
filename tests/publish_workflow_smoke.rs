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
        "build-windows-installers",
        "build-macos-installers",
        "verify-macos-installers-intel",
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

/// Every profile matrix leg (`full`, `hacienda`, `hacienda-legal`) is a required
/// artifact. The Intel leg still uses the dynamic ORT profile, but a failed focus
/// build must now block the whole release instead of being absorbed by
/// `continue-on-error`.
#[test]
fn every_feature_variant_is_a_required_matrix_leg() {
    let workflow = workflow();
    assert!(workflow.contains("features: hacienda\n            variant: hacienda"));
    assert!(workflow.contains("features: hacienda,ort-dynamic\n            variant: hacienda"));
    assert!(workflow.contains("features: hacienda-legal,ort-dynamic\n            variant: hacienda-legal"));
    assert!(workflow.contains("key: ${{ matrix.triple }}-${{ matrix.variant || 'full' }}"));
    assert!(
        !workflow
            .lines()
            .any(|line| line.trim_start().starts_with("continue-on-error:")),
        "no required artifact job may set continue-on-error: any failing combination \
         must block promotion and every registry publish"
    );
}

/// The stable installer asset names come from the Windows/macOS platform specs.
/// They are the public contract; promotion must require all of them on the one
/// tag/release page, for both profiles and all three target shapes.
#[test]
fn finalize_requires_the_installer_assets() {
    let workflow = workflow();
    let block = job_block(&workflow, "finalize_release");
    for asset in [
        // Windows x64 setup.exe (both profiles)
        "basemind-x86_64-pc-windows-msvc-setup.exe",
        "basemind-hacienda-legal-x86_64-pc-windows-msvc-setup.exe",
        // macOS DMG: arm64 and Universal (Intel macOS 15+), both profiles
        "basemind-aarch64-apple-darwin.dmg",
        "basemind-hacienda-legal-aarch64-apple-darwin.dmg",
        "basemind-universal-apple-darwin.dmg",
        "basemind-hacienda-legal-universal-apple-darwin.dmg",
        // the hacienda-legal archives that feed those installers
        "basemind-hacienda-legal-x86_64-pc-windows-msvc.zip",
        "basemind-hacienda-legal-aarch64-apple-darwin.tar.gz",
        "basemind-hacienda-legal-x86_64-apple-darwin.tar.gz",
        "basemind-hacienda-legal-x86_64-unknown-linux-gnu.tar.gz",
        "basemind-hacienda-legal-aarch64-unknown-linux-gnu.tar.gz",
    ] {
        assert!(
            block.contains(asset),
            "finalize_release must require {asset} before promoting a partial release"
        );
    }
}

/// Promotion is fail-closed on job results, not only on asset presence: a failed
/// archive or installer leg leaves `finalize_release` unrun, so no registry can
/// publish. `skipped` is allowed only for the idempotent "already complete" path.
#[test]
fn finalize_is_gated_on_every_builder_result() {
    let workflow = workflow();
    let block = job_block(&workflow, "finalize_release");
    for job in [
        "meta",
        "create_release",
        "build-binaries",
        "build-linux-binaries",
        "build-linux-noavx2",
        "build-windows-installers",
        "build-macos-installers",
        "verify-macos-installers-intel",
        "checksums",
    ] {
        let needle = format!("needs.{job}.result");
        assert!(
            block.contains(&needle),
            "finalize_release must check {needle} so a failed leg blocks the release"
        );
    }
    for job in [
        "create_release",
        "build-binaries",
        "build-linux-binaries",
        "build-linux-noavx2",
        "build-windows-installers",
        "build-macos-installers",
        "verify-macos-installers-intel",
    ] {
        let needle = format!("(needs.{job}.result == 'success' || needs.{job}.result == 'skipped')");
        assert!(
            block.contains(&needle),
            "finalize_release must accept only success or an idempotent skip for {job}"
        );
    }
}

/// Installer jobs address the same draft release by id as the archive matrix —
/// there is still exactly one tag and one GitHub Release page, and `resolve`
/// refuses to scatter assets across ambiguous records.
#[test]
fn installers_upload_to_the_single_release_id() {
    let workflow = workflow();
    for job in ["build-windows-installers", "build-macos-installers"] {
        let block = job_block(&workflow, job);
        assert!(
            block.contains("needs.create_release.outputs.release_id"),
            "job `{job}` must target the one release id, not the tag"
        );
        assert!(
            block.contains("release-asset.sh resolve"),
            "job `{job}` must refuse an ambiguous/promoted release before uploading"
        );
        assert!(
            block.contains("release-asset.sh upload"),
            "job `{job}` must upload its installer to that release"
        );
    }
}

/// Install → launch → uninstall verification (Windows) and DMG structural +
/// runtime verification (macOS) must run in the build jobs, before promotion.
#[test]
fn installers_are_verified_before_promotion() {
    let workflow = workflow();

    let win = job_block(&workflow, "build-windows-installers");
    assert!(
        win.contains("installer/windows/smoke-test.ps1"),
        "Windows installer must be installed and launched via smoke-test.ps1"
    );
    assert!(win.contains("-ExpectedVersion $env:VERSION"));
    for variant in ["full", "hacienda-legal"] {
        assert!(
            win.contains(variant),
            "Windows smoke verification must cover the {variant} profile"
        );
    }

    // macOS: exact worker interfaces, all four DMGs, pinned universal ORT.
    let mac = job_block(&workflow, "build-macos-installers");
    assert!(
        mac.contains("scripts/macos/ort-universal.sh --out dist/libonnxruntime.universal.dylib"),
        "must build the pinned universal dylib with the macOS worker's exact interface"
    );
    assert!(
        mac.contains("scripts/macos/package.sh --skip-tar --variant all --out-dir dist"),
        "must package every profile with the macOS worker's exact interface"
    );
    assert!(
        mac.contains("--fat-ort dist/libonnxruntime.universal.dylib"),
        "the pinned fat dylib must be consumed, never the thin x86_64 one"
    );
    for dmg in [
        "basemind-aarch64-apple-darwin.dmg",
        "basemind-hacienda-legal-aarch64-apple-darwin.dmg",
        "basemind-universal-apple-darwin.dmg",
        "basemind-hacienda-legal-universal-apple-darwin.dmg",
    ] {
        assert!(mac.contains(dmg), "macOS build leg is missing {dmg}");
    }
    assert!(
        mac.contains("tests/macos_dmg_smoke.sh \"$dmg\" arm64"),
        "the arm64 DMGs must be installed and launched before promotion"
    );

    // The universal x86_64 slice declares minos=15.0 exactly; only Intel macOS
    // 15+ can execute it, so that verification is its own fail-closed job.
    let intel = job_block(&workflow, "verify-macos-installers-intel");
    assert!(intel.contains("runs-on: macos-15-intel"));
    assert!(intel.contains("tests/macos_dmg_smoke.sh \"$dmg\" universal"));
    assert!(intel.contains("sw_vers -productVersion"));
    for dmg in [
        "basemind-universal-apple-darwin.dmg",
        "basemind-hacienda-legal-universal-apple-darwin.dmg",
    ] {
        assert!(intel.contains(dmg), "Intel verification is missing {dmg}");
    }
}

/// The universal dylib comes from the pinned PyPI macOS 13 wheels whose SHA-256s
/// the macOS worker reported; no repo-variable dylib path and no min-macos
/// override may weaken the official Intel minimum of macOS 15.
#[test]
fn macos_uses_pinned_ort_and_official_intel_minimum() {
    let workflow = workflow();
    assert!(
        !workflow.contains("--min-macos"),
        "the official minimum is enforced by validate.sh (x86_64 minos exactly 15.0); \
         no workflow override may weaken it"
    );
    assert!(
        !workflow.contains("MACOS_UNIVERSAL_ORT_DYLIB") && !workflow.contains("MACOS_ORT_ARM64_DYLIB"),
        "the old unverified repo-variable dylib path must be gone"
    );
    let mac = job_block(&workflow, "build-macos-installers");
    assert!(mac.contains("scripts/macos/validate.sh arch dist/libonnxruntime.universal.dylib arm64,x86_64"));
    assert!(mac.contains("scripts/macos/validate.sh minos dist/libonnxruntime.universal.dylib x86_64 le 15.0"));
    // The pinned wheel hashes are recorded in the release gates documentation.
    let gates = std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/agents/release.md"))
        .expect("read release gates doc");
    for sha in [
        "6f91d2c9b0965e86827a5ba01531d5b669770b01775b23199565d6c1f136616c",
        "87d8b6eaf0fbeb6835a60a4265fde7a3b60157cf1b2764773ac47237b4d48612",
    ] {
        assert!(
            gates.contains(sha),
            "release gates doc must record pinned ORT sha256 {sha}"
        );
    }
}

/// The checksums file must cover the installers too, or a downloader cannot
/// verify the very artifacts promotion now requires.
#[test]
fn checksums_cover_installers() {
    let workflow = workflow();
    let block = job_block(&workflow, "checksums");
    assert!(block.contains("basemind-*.exe"));
    assert!(block.contains("basemind-*.dmg"));
    assert!(block.contains("(tar\\.gz|zip|exe|dmg)"));
    for asset in [
        "basemind-x86_64-pc-windows-msvc-setup.exe",
        "basemind-universal-apple-darwin.dmg",
    ] {
        assert!(block.contains(asset), "checksum gate is missing {asset}");
    }
}

/// The meta "already published" gate must know about installers, so a draft that
/// lost one reruns the builders instead of being treated as complete.
#[test]
fn complete_release_detection_requires_installers() {
    let workflow = workflow();
    let block = job_block(&workflow, "meta");
    for asset in [
        "basemind-x86_64-pc-windows-msvc-setup.exe",
        "basemind-universal-apple-darwin.dmg",
    ] {
        assert!(block.contains(asset), "meta must count {asset} in release_assets_exist");
    }
}
