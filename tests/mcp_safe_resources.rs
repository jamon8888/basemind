//! `resources/list` and `resources/read` for the `safe/` mirror.
//!
//! A hacienda Safe workspace holds a redacted copy of each original under `<root>/safe/`. An agent
//! whose shell is confined to that folder still needs a way to read it through basemind, and
//! `resources/read` is the standard way: without it the agent got `-32601` and fell back to the
//! shell. The surface must expose `safe/**` and nothing else, so most of these tests are refusals.

use std::path::Path;
use std::process::Command;

use rmcp::ServiceExt;
use rmcp::model::{ErrorCode, ReadResourceRequestParams, ResourceContents};
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

/// A workspace with an original at the root and a redacted mirror under `safe/`.
fn safe_workspace() -> TempDir {
    basemind::store::init_isolated_cache();
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    git(root, &["init", "-q"]);
    git(root, &["config", "commit.gpgsign", "false"]);
    std::fs::write(root.join("original.txt"), "ORIGINAL-SECRET Julie Martin\n").unwrap();
    std::fs::create_dir_all(root.join("safe/sub")).unwrap();
    std::fs::write(root.join("safe/note.md"), "Maître [FULL_NAME_0] à [CITY_0].\n").unwrap();
    std::fs::write(root.join("safe/sub/deep.md"), "deep [EMAIL_0]\n").unwrap();
    std::fs::write(root.join("safe/with space é.md"), "spaced\n").unwrap();
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

/// The JSON-RPC error code a failed read carries.
fn error_code(error: &rmcp::ServiceError) -> Option<ErrorCode> {
    match error {
        rmcp::ServiceError::McpError(data) => Some(data.code),
        _ => None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_server_advertises_resources() {
    let dir = safe_workspace();
    let service = client(dir.path()).await;
    let capabilities = service
        .peer_info()
        .map(|info| info.capabilities.clone())
        .expect("peer info");
    assert!(
        capabilities.resources.is_some(),
        "resources capability must be advertised"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_shows_the_mirror_and_only_the_mirror() {
    let dir = safe_workspace();
    let service = client(dir.path()).await;
    let listed = service.list_all_resources().await.expect("resources/list");
    let uris: Vec<&str> = listed.iter().map(|resource| resource.uri.as_str()).collect();
    assert!(uris.contains(&"basemind://safe/note.md"), "{uris:?}");
    assert!(uris.contains(&"basemind://safe/sub/deep.md"), "{uris:?}");
    assert!(uris.contains(&"basemind://safe/with%20space%20%C3%A9.md"), "{uris:?}");
    assert!(
        uris.iter().all(|uri| uri.starts_with("basemind://safe/")),
        "nothing outside safe/ may be listed: {uris:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_returns_a_mirror_file_including_nested_and_encoded_names() {
    let dir = safe_workspace();
    let service = client(dir.path()).await;
    assert_eq!(
        read(&service, "basemind://safe/note.md").await.unwrap(),
        "Maître [FULL_NAME_0] à [CITY_0].\n"
    );
    assert_eq!(
        read(&service, "basemind://safe/sub/deep.md").await.unwrap(),
        "deep [EMAIL_0]\n"
    );
    assert_eq!(
        read(&service, "basemind://safe/with%20space%20%C3%A9.md")
            .await
            .unwrap(),
        "spaced\n"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_refuses_everything_outside_safe() {
    let dir = safe_workspace();
    let root = dir.path().to_string_lossy().into_owned();
    let service = client(dir.path()).await;
    let refused = [
        "basemind://safe/../original.txt".to_string(),
        "basemind://safe/sub/../../original.txt".to_string(),
        "basemind://safe/%2e%2e/original.txt".to_string(),
        "basemind://safe/..%2foriginal.txt".to_string(),
        "basemind://safe/sub%2f..%2f..%2foriginal.txt".to_string(),
        "basemind://safe//etc/passwd".to_string(),
        format!("basemind://safe/{root}/original.txt"),
        "basemind://safe/..\\original.txt".to_string(),
        "basemind://safe/note.md%00.txt".to_string(),
        "basemind://elsewhere/original.txt".to_string(),
        "basemind://original.txt".to_string(),
        "basemind://safe".to_string(),
        "basemind://safe/".to_string(),
        format!("file://{root}/original.txt"),
        format!("file://{root}/safe/note.md"),
    ];
    for uri in &refused {
        match read(&service, uri).await {
            Ok(text) => panic!("{uri} must be refused but returned {text:?}"),
            Err(error) => {
                let message = error.to_string();
                assert!(
                    !message.contains("ORIGINAL-SECRET"),
                    "{uri}: error leaked the original: {message}"
                );
            }
        }
    }
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_refuses_a_symlink_that_points_out_of_safe() {
    let dir = safe_workspace();
    let root = dir.path();
    std::os::unix::fs::symlink(root.join("original.txt"), root.join("safe/escape.md")).unwrap();
    std::os::unix::fs::symlink(root, root.join("safe/dir-escape")).unwrap();
    let service = client(root).await;
    assert!(
        read(&service, "basemind://safe/escape.md").await.is_err(),
        "file symlink out of safe/"
    );
    assert!(
        read(&service, "basemind://safe/dir-escape/original.txt").await.is_err(),
        "directory symlink out of safe/"
    );
    let listed = service.list_all_resources().await.expect("resources/list");
    assert!(
        listed.iter().all(|resource| !resource.uri.contains("escape")),
        "a symlink out of safe/ must not be listed: {:?}",
        listed.iter().map(|resource| &resource.uri).collect::<Vec<_>>()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_reports_a_missing_file_and_a_directory_as_errors() {
    let dir = safe_workspace();
    let service = client(dir.path()).await;
    let missing = read(&service, "basemind://safe/nope.md")
        .await
        .expect_err("missing file");
    assert_eq!(error_code(&missing), Some(ErrorCode::RESOURCE_NOT_FOUND));
    assert!(
        read(&service, "basemind://safe/sub").await.is_err(),
        "a directory is not a resource"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_refuses_a_file_over_the_size_cap() {
    let dir = safe_workspace();
    std::fs::write(dir.path().join("safe/big.md"), vec![b'x'; (1 << 20) + 1]).unwrap();
    std::fs::write(dir.path().join("safe/just-fits.md"), vec![b'y'; 1 << 20]).unwrap();
    let service = client(dir.path()).await;
    assert!(
        read(&service, "basemind://safe/big.md").await.is_err(),
        "over the 1 MiB cap"
    );
    assert_eq!(
        read(&service, "basemind://safe/just-fits.md").await.unwrap().len(),
        1 << 20
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_refuses_a_file_that_is_not_text() {
    let dir = safe_workspace();
    std::fs::write(dir.path().join("safe/blob.md"), [0xff, 0xfe, 0x00, 0x80]).unwrap();
    let service = client(dir.path()).await;
    assert!(read(&service, "basemind://safe/blob.md").await.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_workspace_without_safe_lists_nothing_and_reads_nothing() {
    basemind::store::init_isolated_cache();
    let dir = tempfile::tempdir().expect("tempdir");
    git(dir.path(), &["init", "-q"]);
    std::fs::write(dir.path().join("original.txt"), "ORIGINAL-SECRET").unwrap();
    let service = client(dir.path()).await;
    assert!(service.list_all_resources().await.expect("resources/list").is_empty());
    assert!(read(&service, "basemind://safe/original.txt").await.is_err());
}
