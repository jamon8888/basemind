//! Static contract checks for the Windows user installers, plus an opt-in driver
//! for the real Windows end-to-end smoke test.
//!
//! The static part runs on every platform (no Windows toolchain needed) and guards
//! the invariants that make the two setup.exe artifacts installable: per-user
//! scope, PATH + config handling, CLI/MCP-only payload, and preservation of the
//! existing release .zip. The live part runs only on Windows when a built setup
//! executable is supplied through BASEMIND_SETUP_EXE.

use std::path::{Path, PathBuf};

fn repo_path(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

fn read(relative: &str) -> String {
    let path = repo_path(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
}

const FULL_ARTIFACT: &str = "basemind-x86_64-pc-windows-msvc-setup.exe";
const LEGAL_ARTIFACT: &str = "basemind-hacienda-legal-x86_64-pc-windows-msvc-setup.exe";

#[test]
fn installer_sources_exist() {
    for file in [
        "installer/windows/basemind.iss",
        "installer/windows/build-installer.ps1",
        "installer/windows/build-installers.sh",
        "installer/windows/smoke-test.ps1",
        "installer/windows/VALIDATION.md",
        "docs/windows/installer.md",
    ] {
        assert!(repo_path(file).is_file(), "missing Windows installer source: {file}");
    }
}

#[test]
fn build_driver_produces_both_named_artifacts() {
    let ps1 = read("installer/windows/build-installer.ps1");
    let sh = read("installer/windows/build-installers.sh");
    // The build driver composes names from the triple + variant; the wrapper
    // asserts the exact literal names afterwards.
    assert!(
        ps1.contains("basemind-$Triple-setup.exe"),
        "full artifact name is not composed in build-installer.ps1"
    );
    assert!(
        ps1.contains("basemind-hacienda-legal-$Triple-setup.exe"),
        "legal artifact name is not composed in build-installer.ps1"
    );
    for artifact in [FULL_ARTIFACT, LEGAL_ARTIFACT] {
        assert!(
            sh.contains(artifact),
            "{artifact} is not verified by build-installers.sh"
        );
    }
    assert!(
        ps1.contains("hacienda-legal") && ps1.contains("full"),
        "build-installer.ps1 must accept both profiles"
    );
}

#[test]
fn inno_script_installs_cli_mcp_only_per_user() {
    let iss = read("installer/windows/basemind.iss");

    // Per-user: no elevation, no machine-wide directory.
    assert!(iss.contains("PrivilegesRequired=lowest"));
    assert!(iss.contains("{localappdata}"));

    // CLI + stdio MCP payload: only basemind.exe and its DLLs, no GUI, no icons.
    assert!(iss.contains("basemind.exe"));
    assert!(
        !iss.lines().any(|line| line.trim() == "[Icons]"),
        "installer must not create GUI shortcuts"
    );
    assert!(iss.contains("stdio MCP") || iss.contains("CLI + MCP"));

    // The two variants are separately upgradeable and driven by the build define.
    assert!(iss.matches("AppId").count() >= 2, "both profiles need a stable AppId");
    assert!(iss.contains("LegalVariant"), "profile switch must be build-driven");

    // PATH handling: write to HKCU\Environment\Path, idempotent, with uninstall removal.
    assert!(iss.contains("HKEY_CURRENT_USER"));
    assert!(iss.contains("Environment"));
    assert!(iss.contains("PathValueName"));
    assert!(iss.contains("PathContainsDir"));
    assert!(iss.contains("RegWriteStringValue"));
    assert!(iss.contains("CurStepChanged"));
    assert!(iss.contains("CurUninstallStepChanged"));

    // Config template is shipped under %APPDATA%, per profile, without clobbering edits.
    assert!(iss.contains("{appdata}\\Basemind"));
    assert!(iss.contains("onlyifdoesntexist"));
}

#[test]
fn release_zip_is_preserved() {
    let packager = read("scripts/package-release.sh");
    assert!(
        packager.contains("basemind-${TRIPLE}.zip") || packager.contains("${NAME}-${TRIPLE}.zip"),
        "package-release.sh must keep producing the Windows .zip archive"
    );
    let wrapper = read("installer/windows/build-installers.sh");
    assert!(
        wrapper.contains("fingerprint") && wrapper.contains("ZIP_FULL"),
        "build wrapper must prefer and fingerprint the release archives"
    );
    let docs = read("docs/windows/installer.md");
    assert!(
        docs.contains("preserved") || docs.contains("unchanged"),
        "Windows docs must state the .zip is preserved"
    );
}

#[test]
fn smoke_test_covers_install_path_config_launch_and_uninstall() {
    let ps1 = read("installer/windows/smoke-test.ps1");
    for needle in [
        "--version",
        "serve --help",
        "HKCU:\\Environment",
        "basemind.toml",
        "unins000.exe",
        "Profile:",
    ] {
        assert!(ps1.contains(needle), "smoke test is missing check: {needle}");
    }
}

/// The real end-to-end check. Gated because it needs a Windows host, Inno Setup,
/// and a built setup.exe; the static checks above run everywhere.
#[cfg(windows)]
#[test]
#[ignore = "requires BASEMIND_SETUP_EXE (see docs/windows/installer.md)"]
fn windows_setup_end_to_end_when_provided() {
    let Ok(setup) = std::env::var("BASEMIND_SETUP_EXE") else {
        eprintln!("BASEMIND_SETUP_EXE not set; skipping live Windows smoke test");
        return;
    };
    let variant = std::env::var("BASEMIND_SETUP_VARIANT").unwrap_or_else(|_| "full".to_string());
    let version = std::env::var("BASEMIND_SETUP_VERSION").unwrap_or_default();

    let mut args = vec![
        "-NoProfile".to_string(),
        "-ExecutionPolicy".to_string(),
        "Bypass".to_string(),
        "-File".to_string(),
        repo_path("installer/windows/smoke-test.ps1")
            .to_string_lossy()
            .into_owned(),
        "-SetupExe".to_string(),
        setup,
        "-Variant".to_string(),
        variant,
    ];
    if !version.is_empty() {
        args.push("-ExpectedVersion".to_string());
        args.push(version);
    }

    let status = std::process::Command::new("powershell")
        .args(&args)
        .status()
        .expect("run smoke-test.ps1 with powershell");
    assert!(status.success(), "Windows installer smoke test failed");
}
