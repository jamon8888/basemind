//! Canary for `.github/workflows/release-assets.yaml` (PR #190): the asset
//! matrix, required-asset set and fail-closed job graph must move together.
//! Structural assertions over the workflow text only, in the style of
//! `tests/publish_workflow_smoke.rs`, so no YAML dependency is needed.

use std::path::PathBuf;

fn workflow() -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".github/workflows/release-assets.yaml");
    std::fs::read_to_string(&path)
        .expect("read .github/workflows/release-assets.yaml")
        .replace("\r\n", "\n")
}

/// The body of a top-level job: from its 2-space-indented header to the next
/// 2-space-indented job header. Steps live at deeper indentation, so the only
/// 2-space non-space lines are sibling job headers.
fn job_block<'a>(workflow: &'a str, job: &str) -> &'a str {
    let marker = format!("\n  {job}:\n");
    let start = workflow
        .find(&marker)
        .unwrap_or_else(|| panic!("job `{job}` not found in release-assets.yaml"))
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

/// Nothing that consumes assets may run before every archive leg succeeds, and
/// checksums must require the installer legs too.
#[test]
fn job_graph_is_fail_closed() {
    let w = workflow();
    assert!(job_block(&w, "archives").contains("resolve-source"));
    for installer in ["windows-installers", "macos-installers"] {
        let block = job_block(&w, installer);
        assert!(block.contains("resolve-source") && block.contains("archives"));
        assert!(block.contains("Verify version against Cargo.toml"));
    }
    assert!(job_block(&w, "verify-macos-installers-intel").contains("macos-installers"));
    let checksums = job_block(&w, "checksums");
    for dep in [
        "resolve-source",
        "archives",
        "windows-installers",
        "macos-installers",
        "verify-macos-installers-intel",
    ] {
        assert!(checksums.contains(dep), "checksums must depend on {dep}");
    }
    assert!(checksums.contains("Fail closed on a partial asset set"));
}

/// This workflow builds artifacts only: a release mutation or registry push
/// would break the non-publishing contract.
#[test]
fn workflow_does_not_mutate_a_release_or_publish() {
    let w = workflow();
    for forbidden in [
        "gh release",
        "gh api",
        "cargo publish",
        "npm publish",
        "twine upload",
        "brew push",
        "contents: write",
    ] {
        assert!(
            !w.contains(forbidden),
            "non-publishing workflow must not contain `{forbidden}`"
        );
    }
    assert!(w.contains("contents: read"));
}

/// Every required checksum asset must be produced by a matrix leg, and both
/// profiles plus the Intel dynamic-ORT leg must stay in that matrix.
#[test]
fn required_assets_match_the_build_matrix() {
    let w = workflow();
    let required = [
        "basemind-aarch64-apple-darwin.tar.gz",
        "basemind-x86_64-apple-darwin.tar.gz",
        "basemind-x86_64-pc-windows-msvc.zip",
        "basemind-hacienda-legal-aarch64-apple-darwin.tar.gz",
        "basemind-hacienda-legal-x86_64-apple-darwin.tar.gz",
        "basemind-hacienda-legal-x86_64-pc-windows-msvc.zip",
        "basemind-x86_64-pc-windows-msvc-setup.exe",
        "basemind-hacienda-legal-x86_64-pc-windows-msvc-setup.exe",
        "basemind-aarch64-apple-darwin.dmg",
        "basemind-hacienda-legal-aarch64-apple-darwin.dmg",
        "basemind-universal-apple-darwin.dmg",
        "basemind-hacienda-legal-universal-apple-darwin.dmg",
    ];
    let block = job_block(&w, "checksums");
    for asset in required {
        assert!(block.contains(asset), "checksums must require {asset}");
    }
    let archives = job_block(&w, "archives");
    for leg in [
        "aarch64-apple-darwin",
        "x86_64-apple-darwin",
        "x86_64-pc-windows-msvc",
        "hacienda-legal",
        "full,ort-dynamic",
    ] {
        assert!(archives.contains(leg), "archive matrix lost leg `{leg}`");
    }
}

/// A dispatched ref must travel through the environment, never be interpolated
/// into the shell script.
#[test]
fn dispatched_ref_is_not_interpolated_into_shell() {
    let w = workflow();
    let block = job_block(&w, "resolve-source");
    assert!(block.contains("SOURCE_REF: ${{ inputs.source_ref || github.sha }}"));
    assert!(block.contains("ref=\"$SOURCE_REF\""));
    assert!(!block.contains("ref=\"${{"));
}

#[test]
fn default_asset_version_matches_cargo_toml() {
    let w = workflow();
    let default = w
        .lines()
        .find_map(|line| line.strip_prefix("        default: \""))
        .expect("workflow dispatch version default")
        .trim_end_matches('"');
    let cargo =
        std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")).unwrap_or_default();
    let version = cargo
        .lines()
        .find_map(|line| line.strip_prefix("version = \""))
        .unwrap_or("missing")
        .trim_end_matches('"');
    assert_eq!(default, version);
}
