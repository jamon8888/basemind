//! The protected surface: a workspace that carries a `safe/` mirror withholds the two tools
//! that hand out the true values, and the upstream resources that are not masked by the mirror.
//!
//! The mirror is the marker: the privacy gateway writes `safe/` when it writes the redacted
//! copy, so its presence is what turns the boundary on. Without it, basemind is a plain code
//! index and every tool is served as before.

use std::path::Path;
use std::process::Command;

use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, ReadResourceRequestParams, ResourceContents};
use tempfile::TempDir;

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(repo)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@e.x")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@e.x")
        .status()
        .expect("git in PATH");
    assert!(status.success(), "git {args:?} failed");
}

/// A workspace, with or without the `safe/` mirror the gateway writes.
fn workspace(with_mirror: bool) -> TempDir {
    basemind::store::init_isolated_cache();
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    git(root, &["init", "-q"]);
    git(root, &["config", "commit.gpgsign", "false"]);
    std::fs::write(root.join("original.txt"), "ORIGINAL-SECRET Julie Martin\n").unwrap();
    if with_mirror {
        std::fs::create_dir_all(root.join("safe")).unwrap();
        std::fs::write(root.join("safe/note.md"), "Maître [FULL_NAME_0] à [CITY_0].\n").unwrap();
    }
    dir
}

async fn client(root: &Path) -> rmcp::service::RunningService<rmcp::RoleClient, ()> {
    let transport = basemind::mcp::serve_in_memory(root, "working")
        .await
        .expect("in-memory serve");
    ().serve(transport).await.expect("rmcp handshake")
}

async fn read(
    service: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    uri: &str,
) -> Result<String, rmcp::ServiceError> {
    let result = service.read_resource(ReadResourceRequestParams::new(uri)).await?;
    Ok(result
        .contents
        .iter()
        .filter_map(|content| match content {
            ResourceContents::TextResourceContents { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(""))
}

#[cfg(feature = "documents")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_two_tools_are_withheld_in_a_protected_workspace() {
    let dir = workspace(true);
    let service = client(dir.path()).await;
    let listed = service.list_all_tools().await.expect("tools/list");
    let names: Vec<&str> = listed.iter().map(|tool| tool.name.as_ref()).collect();
    assert!(!names.contains(&"redact_text"), "{names:?}");
    assert!(!names.contains(&"vault"), "{names:?}");
    // The rest of the surface is untouched.
    assert!(names.contains(&"code"), "{names:?}");
}

#[cfg(feature = "documents")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_two_tools_are_served_in_an_ordinary_workspace() {
    let dir = workspace(false);
    let service = client(dir.path()).await;
    let listed = service.list_all_tools().await.expect("tools/list");
    let names: Vec<&str> = listed.iter().map(|tool| tool.name.as_ref()).collect();
    assert!(names.contains(&"redact_text"), "{names:?}");
    assert!(names.contains(&"vault"), "{names:?}");
}

#[cfg(feature = "documents")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn calling_a_withheld_tool_is_refused() {
    let dir = workspace(true);
    let service = client(dir.path()).await;
    for tool in ["redact_text", "vault"] {
        let error = service
            .call_tool(CallToolRequestParams::new(tool).with_arguments(serde_json::Map::new()))
            .await
            .expect_err("a withheld tool must be refused");
        let message = error.to_string();
        assert!(message.contains("protected workspace"), "{tool}: {message}");
    }
}

#[cfg(feature = "documents")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_mirror_is_still_readable_in_a_protected_workspace() {
    let dir = workspace(true);
    let service = client(dir.path()).await;
    assert_eq!(
        read(&service, "basemind://safe/note.md").await.unwrap(),
        "Maître [FULL_NAME_0] à [CITY_0].\n"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_upstream_resources_are_refused_in_a_protected_workspace() {
    let dir = workspace(true);
    let service = client(dir.path()).await;
    for uri in [
        "basemind://status",
        "basemind://repo/map",
        "basemind://outline/src/main.rs",
        "basemind://memory/skill/one",
    ] {
        let error = read(&service, uri)
            .await
            .expect_err("the upstream resources must be refused");
        let message = error.to_string();
        assert!(message.contains("protected workspace"), "{uri}: {message}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_upstream_resources_are_served_in_an_ordinary_workspace() {
    let dir = workspace(false);
    let service = client(dir.path()).await;
    let status = read(&service, "basemind://status").await.expect("status resource");
    assert!(!status.is_empty());
}

/// The CLI half of the same boundary: `basemind redact` and `basemind vault` are refused in a
/// protected workspace, and the gateway's capability re-opens them.
#[cfg(feature = "documents")]
#[test]
fn the_cli_refuses_the_two_commands_in_a_protected_workspace() {
    let dir = workspace(true);
    let root = dir.path().to_str().unwrap().to_string();

    let refused = Command::new(env!("CARGO_BIN_EXE_basemind"))
        .args(["--root", &root, "redact", "--text", "Julie Martin"])
        .output()
        .expect("run basemind redact");
    assert_eq!(
        refused.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    let message = String::from_utf8_lossy(&refused.stderr);
    assert!(message.contains("protected workspace"), "{message}");

    let refused = Command::new(env!("CARGO_BIN_EXE_basemind"))
        .args(["--root", &root, "vault", "inspect"])
        .output()
        .expect("run basemind vault");
    assert_eq!(
        refused.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
}

#[cfg(feature = "documents")]
#[test]
fn the_gateway_capability_re_opens_the_cli() {
    let dir = workspace(true);
    let root = dir.path().to_str().unwrap().to_string();
    let output = Command::new(env!("CARGO_BIN_EXE_basemind"))
        .args(["--root", &root, "redact", "--text", "Julie Martin"])
        .env("BASEMIND_GATEWAY_TOKEN", "a-secret-the-gateway-holds")
        .output()
        .expect("run basemind redact");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("redacted_text"), "{stdout}");
}
