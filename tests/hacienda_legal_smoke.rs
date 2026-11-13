//! The `hacienda-legal` focus release: a documents-only build with rehydration.
//!
//! Four properties, all but the fourth asserted against the real MCP surface (in-memory serve + rmcp
//! client, same harness as `mcp_protected_surface.rs`):
//!
//! 1. **Surface gating.** The build advertises `vault`, `redact_text` and the `memory`
//!    domain, and does NOT advertise the excluded domains (`web`, `agents`, `workspace`,
//!    `shell`) — a client folder gets the document stack, not the broker/shell surface.
//! 2. **The legal pipeline end to end.** A client memo is ingested by a scan with
//!    ingestion-time redaction on, found by `memory mode=documents` (lexical lane —
//!    `embed = false`, so no model is downloaded), `redact_text` returns a rehydration
//!    map, and `vault` encrypt → decrypt → find round-trips it.
//! 3. **The shipped preset parses.** `examples/basemind-hacienda-legal.toml` must stay a
//!    valid `basemind.toml` — it is the file operators drop into a client folder.
//! 4. **`serve` answers over stdio.** Without `comms`, the shipped binary's `serve` runs the
//!    server in process; spawned for real, it must answer `initialize`.

#![cfg(feature = "hacienda-legal")]

use std::path::Path;
use std::process::Command;

use rmcp::ServiceExt;
use rmcp::model::{CallToolRequestParams, CallToolResult, ContentBlock};
use serde_json::{Value, json};
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

/// A client-folder workspace: lexical-only documents (no embedder download in CI),
/// ingestion-time redaction with the reversible token strategy, one lease memo
/// carrying PII.
fn workspace() -> TempDir {
    basemind::store::init_isolated_cache();
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    git(root, &["init", "-q"]);
    git(root, &["config", "commit.gpgsign", "false"]);
    std::fs::write(
        root.join("basemind.toml"),
        "\"$schema\" = \"v1\"\n\n[documents]\nembed = false\n\n[documents.redaction]\nenabled = true\nstrategy = \"token-replace\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("matter-24-0187")).unwrap();
    std::fs::write(
        root.join("matter-24-0187/lease.md"),
        "# Lease — Matter 24-0187\n\nIV. TERMINATION\n\nThe landlord may terminate for \
         non-payment at any time once the cure period has expired.\n\nContact: \
         alice.smith@example.com, +33 6 12 34 56 78.\n",
    )
    .unwrap();
    dir
}

fn run_scan(root: &Path) {
    let cfg = basemind::config::load(root).expect("load basemind.toml");
    assert!(!cfg.documents.embed, "this test runs the lexical-only lane on purpose");
    assert!(cfg.documents.redaction.enabled, "ingestion-time redaction is the point");
    let output = Command::new(env!("CARGO_BIN_EXE_basemind"))
        .args(["--root", root.to_str().expect("workspace path"), "scan"])
        .output()
        .expect("basemind scan CLI");
    assert!(
        output.status.success(),
        "the legal workspace must scan through the shipped CLI: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

async fn client(root: &Path) -> rmcp::service::RunningService<rmcp::RoleClient, ()> {
    // `run_scan` goes through the CLI, which canonicalizes `--root`; serve the same canonical path.
    // On macOS the tempdir sits under `/var`, a symlink to `/private/var`, and a server rooted at
    // the raw path found no document hits for the CLI-scanned workspace.
    let root = root.canonicalize().expect("canonical workspace root");
    let transport = basemind::mcp::serve_in_memory_lean(&root, "working", false)
        .await
        .expect("in-memory serve");
    ().serve(transport).await.expect("rmcp handshake")
}

async fn call(
    service: &rmcp::service::RunningService<rmcp::RoleClient, ()>,
    tool: &'static str,
    arguments: Value,
) -> Result<CallToolResult, rmcp::ServiceError> {
    service
        .call_tool(
            CallToolRequestParams::new(tool).with_arguments(arguments.as_object().expect("arguments object").clone()),
        )
        .await
}

fn payload(result: &CallToolResult) -> Value {
    let raw = result
        .content
        .iter()
        .find_map(|c| match c {
            ContentBlock::Text(t) => Some(t.text.clone()),
            _ => None,
        })
        .unwrap_or_default();
    serde_json::from_str(&raw).unwrap_or(Value::Null)
}

/// The focus build advertises the document/privacy surface and withholds the
/// excluded domains — both in the MCP tool list and in the `domain_modes` parity list.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_focus_surface_advertises_documents_and_rehydration_only() {
    let dir = workspace();
    let service = client(dir.path()).await;

    let listed = service.list_all_tools().await.expect("tools/list");
    let names: Vec<&str> = listed.iter().map(|tool| tool.name.as_ref()).collect();
    for expected in ["memory", "vault", "redact_text", "admin", "code", "git", "graph"] {
        assert!(names.contains(&expected), "{expected} must be advertised: {names:?}");
    }
    for (domain, enabled) in [
        ("web", cfg!(feature = "crawl")),
        ("agents", cfg!(feature = "comms")),
        ("workspace", cfg!(feature = "comms")),
        ("shell", cfg!(feature = "shells")),
    ] {
        assert_eq!(
            names.contains(&domain),
            enabled,
            "feature/domain mismatch for {domain}: {names:?}"
        );
    }

    let domains: Vec<&str> = basemind::mcp::mode::domain_modes()
        .iter()
        .map(|(domain, _)| *domain)
        .collect();
    assert!(domains.contains(&"vault"), "vault domain advertised: {domains:?}");
    assert!(domains.contains(&"memory"), "memory domain advertised: {domains:?}");
    for (domain, enabled) in [
        ("web", cfg!(feature = "crawl")),
        ("agents", cfg!(feature = "comms")),
        ("workspace", cfg!(feature = "comms")),
        ("shell", cfg!(feature = "shells")),
    ] {
        assert_eq!(
            domains.contains(&domain),
            enabled,
            "feature/domain mismatch for {domain}: {domains:?}"
        );
    }
}

/// Ingest → search → redact → vault round-trip over the MCP surface.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ingest_search_redact_and_rehydrate_end_to_end() {
    let dir = workspace();
    run_scan(dir.path());
    let service = client(dir.path()).await;

    // RAG search over the ingested memo (lexical lane, no embedder).
    let result = call(
        &service,
        "memory",
        json!({ "mode": "documents", "query": "cure period termination landlord", "limit": 5 }),
    )
    .await
    .expect("memory documents call");
    let hits = payload(&result);
    let hits_text = serde_json::to_string(&hits).expect("serialize hits");
    assert!(
        hits_text.contains("matter-24-0187/lease.md"),
        "the ingested lease must be found by the documents lane: {hits_text}"
    );
    assert!(
        !hits_text.contains("alice.smith@example.com"),
        "scan-time redaction must have replaced the PII before indexing: {hits_text}"
    );

    // Redact arbitrary text; the rehydration map comes back to the caller.
    let result = call(
        &service,
        "redact_text",
        json!({ "text": "Reach alice.smith@example.com or +33 6 12 34 56 78 about the lease." }),
    )
    .await
    .expect("redact_text call");
    let redacted = payload(&result);
    let redacted_text = redacted["redacted_text"].as_str().expect("redacted_text");
    assert!(
        !redacted_text.contains("alice.smith@example.com"),
        "the email must be tokenised away: {redacted_text}"
    );
    assert!(
        redacted_text.starts_with("Reach "),
        "surrounding contact text must remain: {redacted_text}"
    );
    assert!(
        redacted_text.ends_with(" about the lease."),
        "surrounding contact text must remain: {redacted_text}"
    );
    assert!(
        !redacted_text.contains("+33 6 12 34 56 78"),
        "the phone must be tokenised away: {redacted_text}"
    );
    let map = redacted["rehydration_map"]
        .as_object()
        .expect("rehydration_map object")
        .clone();
    assert!(!map.is_empty(), "the rehydration map must capture the original values");
    assert!(
        map.values().any(|v| v == "alice.smith@example.com"),
        "the map must hold the email verbatim for later rehydration: {map:?}"
    );

    // Vault: encrypt the map with a passphrase, decrypt it back, find a subject in it.
    let passphrase = "matter-24-0187-passphrase";
    let result = call(
        &service,
        "vault",
        json!({ "mode": "encrypt", "map": map, "passphrase": passphrase }),
    )
    .await
    .expect("vault encrypt");
    let encrypted = payload(&result);
    let blob = encrypted["encrypted_blob"]
        .as_str()
        .expect("encrypted_blob")
        .to_string();
    assert_eq!(encrypted["token_count"].as_u64(), Some(map.len() as u64));

    let result = call(
        &service,
        "vault",
        json!({ "mode": "decrypt", "encrypted_blob": blob, "passphrase": passphrase }),
    )
    .await
    .expect("vault decrypt");
    let decrypted = payload(&result);
    let round_tripped: serde_json::Map<String, Value> = decrypted["map"]
        .as_object()
        .expect("decrypted map")
        .clone()
        .into_iter()
        .collect();
    assert_eq!(
        round_tripped, map,
        "decrypt(encrypt(map)) must return the identical rehydration map"
    );

    let result = call(
        &service,
        "vault",
        json!({ "mode": "find", "map": map, "query": "alice.smith@example.com" }),
    )
    .await
    .expect("vault find");
    let found = payload(&result);
    assert!(
        found["matches"].as_array().is_some_and(|m| !m.is_empty()),
        "vault find must locate the subject by its original value: {found}"
    );
}

/// The shipped legal preset must parse as a valid `basemind.toml` and carry the
/// focus-release semantics (redaction on with the reversible strategy, NER on,
/// graphrag on, multilingual embeddings).
#[test]
fn the_shipped_legal_preset_parses_and_enables_the_legal_stack() {
    let raw = include_str!("../examples/basemind-hacienda-legal.toml");
    let cfg = basemind::config::parse_str(raw).expect("preset must parse as a basemind.toml");
    assert!(cfg.documents.enabled);
    assert!(cfg.documents.embed, "the preset ships the vector lane on");
    assert_eq!(cfg.documents.embedding_preset, "multilingual");
    assert!(cfg.documents.redaction.enabled);
    assert!(
        cfg.documents.redaction.categories.iter().any(|c| c == "iban"),
        "legal PII categories must include iban: {:?}",
        cfg.documents.redaction.categories
    );
    assert!(cfg.documents.ner.enabled);
    assert!(
        !cfg.documents.ner.custom_labels.is_empty(),
        "legal entity labels configured"
    );
    assert!(cfg.documents.graphrag.enabled);
    assert!(
        cfg.documents.citations.extract,
        "the exact lane covers statute/docket cites"
    );
}

/// 4. **`serve` answers over stdio.** Without `comms` there is no daemon to relay to: `basemind serve`
///    runs the server in process. The in-memory harness above never goes through that path, so this
///    spawns the shipped binary, sends `initialize` and expects a result instead of a startup panic.
#[cfg(not(feature = "comms"))]
#[test]
fn serve_answers_initialize_over_stdio_without_a_daemon() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::Stdio;
    use std::time::Duration;

    basemind::store::init_isolated_cache();
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path().canonicalize().expect("canonical workspace root");
    // A bare tempdir is neither a git repository nor holds `basemind.toml`, so `serve` would refuse it.
    std::fs::write(root.join("basemind.toml"), "\"$schema\" = \"v1\"\n").expect("write project marker");
    let mut child = Command::new(env!("CARGO_BIN_EXE_basemind"))
        .args(["serve", "--root", root.to_str().expect("workspace path")])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn basemind serve");
    let mut stdin = child.stdin.take().expect("serve stdin");
    let initialize = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "hacienda-legal-smoke", "version": "0"}
        }
    });
    writeln!(stdin, "{initialize}").expect("write initialize");
    stdin.flush().expect("flush initialize");

    let stdout = child.stdout.take().expect("serve stdout");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = BufReader::new(stdout).read_line(&mut line);
        let _ = tx.send(line);
    });
    let line = rx.recv_timeout(Duration::from_secs(60)).unwrap_or_default();
    drop(stdin);
    let _ = child.kill();
    let output = child.wait_with_output().expect("wait for serve");
    let response: Value = serde_json::from_str(line.trim()).unwrap_or_else(|e| {
        panic!(
            "serve must answer initialize over stdio ({e}); stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        )
    });
    assert_eq!(response["id"], json!(1), "{response}");
    assert!(
        response["result"]["serverInfo"].is_object(),
        "initialize must return serverInfo: {response}"
    );
}
