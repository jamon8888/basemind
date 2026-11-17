//! The protected surface: a workspace that carries a `safe/` mirror withholds the two tools
//! that hand out the true values, and the upstream resources that are not masked by the mirror.
//! The `memory` tool stays callable, but its shared tier is refused the same way: private
//! `individual` memory passes, and so does the `documents` search over the masked index.
//!
//! The mirror is the marker: the privacy gateway writes `safe/` when it writes the redacted
//! copy, so its presence is what turns the boundary on. Without it, basemind is a plain code
//! index and every tool is served as before.

use std::path::Path;
use std::process::Command;

use rmcp::ServiceExt;
#[cfg(feature = "documents")]
use rmcp::model::CallToolRequestParams;
use rmcp::model::{ReadResourceRequestParams, ResourceContents};
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

/// Call the `memory` tool with the given arguments and return its protocol outcome.
#[cfg(feature = "documents")]
async fn memory(
    service: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    arguments: serde_json::Value,
) -> Result<rmcp::model::CallToolResult, rmcp::ServiceError> {
    service
        .call_tool(
            CallToolRequestParams::new("memory")
                .with_arguments(arguments.as_object().expect("memory arguments").clone()),
        )
        .await
}

#[cfg(feature = "documents")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn memory_on_the_shared_tier_is_refused_in_a_protected_workspace() {
    // The shared tier is exactly what the withheld `basemind://memory/{key}` resource
    // serves: the tool cannot hand out the same unmasked content.
    let dir = workspace(true);
    let service = client(dir.path()).await;
    for call in [
        serde_json::json!({ "mode": "get", "key": "skill/one" }),
        serde_json::json!({ "mode": "list" }),
        serde_json::json!({ "mode": "search", "query": "notes" }),
        serde_json::json!({ "mode": "put", "key": "skill/one", "value": "v" }),
        serde_json::json!({ "mode": "audit" }),
        serde_json::json!({ "mode": "proposals" }),
    ] {
        let error = memory(&service, call)
            .await
            .expect_err("a shared-tier memory call must be refused");
        assert!(
            error.to_string().contains("protected workspace"),
            "the refusal must name the boundary: {error}"
        );
    }
}

/// The private roundtrip needs the store backend itself: `put` refuses with "requires the
/// `memory` feature" when the binary carries `documents` without `memory` (the CI legs that
/// compile this file — `full` — always build both).
#[cfg(all(feature = "documents", feature = "memory"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_private_memory_tier_stays_available_in_a_protected_workspace() {
    let dir = workspace(true);
    let service = client(dir.path()).await;
    memory(
        &service,
        serde_json::json!({
            "mode": "put",
            "key": "agent/scratch",
            "value": "reviser la section 3",
            "embed": false,
            "visibility": "individual",
        }),
    )
    .await
    .expect("an individual put stays available");
    let result = memory(
        &service,
        serde_json::json!({
            "mode": "get",
            "key": "agent/scratch",
            "visibility": "individual",
        }),
    )
    .await
    .expect("an individual get stays available");
    let text = result
        .content
        .iter()
        .find_map(|content| match content {
            rmcp::model::ContentBlock::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .unwrap_or_default();
    assert!(
        text.contains("reviser la section 3"),
        "the private note round-trips: {text}"
    );
}

#[cfg(feature = "documents")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_lean_wrapper_gates_memory_on_the_shared_tier_too() {
    let dir = workspace(true);
    let transport = basemind::mcp::serve_in_memory_lean(dir.path(), "working", true)
        .await
        .expect("lean in-memory serve");
    let service = ().serve(transport).await.expect("rmcp handshake");
    let error = service
        .call_tool(
            CallToolRequestParams::new("invoke_tool").with_arguments(
                serde_json::json!({
                    "tool_name": "memory",
                    "tool_input": { "mode": "get", "key": "skill/one" },
                })
                .as_object()
                .expect("invoke arguments")
                .clone(),
            ),
        )
        .await
        .expect_err("a lean wrapper cannot carry the call past the gate");
    assert!(
        error.to_string().contains("protected workspace"),
        "the refusal must name the boundary: {error}"
    );
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
