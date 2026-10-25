//! Protected-workspace detection: the boundary between the agent and the true values.
//!
//! A workspace that carries a `safe/` mirror is **protected**: the mirror holds the redacted
//! copy of the originals, and the agent is confined to it. Inside such a workspace the
//! agent-facing surface must not offer the two operations that hand out the true values:
//!
//! - `redact_text` returns the token→original table and every detection in the clear;
//! - `vault` encrypts, decrypts and searches that same table.
//!
//! In the gateway (passerelle) architecture those values belong to the gateway alone: it
//! keeps the keys in the system keychain and hands the agent nothing but tokens. basemind
//! stays the engine — detection, extraction, the `safe/` mirror and the index — so the two
//! tools are withheld from the agent without being removed from the engine.
//!
//! The mirror is the marker, not a configuration key: the gateway creates `safe/` when it
//! writes the mirror, so its presence is the one fact both sides already agree on. A `safe`
//! entry of any kind (a directory, a file, a symlink) marks the workspace as protected —
//! the check fails closed, because a mirror that points somewhere else is exactly the case
//! where withholding matters most.
//!
//! The gateway still needs the engine, so it marks the basemind process it owns with
//! [`GATEWAY_ENV`]. That is a capability rather than a flag: the value is a secret the
//! gateway generates, and keeping it out of the agent's sandbox environment is the
//! gateway's job — an agent that inherits it can call the two tools.
//!
//! The capability does what a process-level switch can and nothing more: it separates a
//! basemind process the gateway launched from configuration from any other. An agent able
//! to spawn its own processes can launch a basemind without the mirror's protections —
//! no environment check can tell a spawner the gateway chose from a spawner the agent
//! chose, because both run under the agent's own OS credentials (anything the gateway can
//! hold, those processes could read). Halting that case is the agent runtime's sandbox,
//! which the gateway provisions; ADR-0015 records the residual risk on purpose.

use std::path::Path;

/// The mirror directory that marks a workspace as protected.
pub const MIRROR_DIR: &str = "safe";

/// Environment variable the privacy gateway sets on the basemind process it owns.
///
/// Its presence (non-empty) marks the process as the gateway's own, which re-opens the two
/// withheld tools for it. It is deliberately not a boolean: the gateway generates a random
/// value, so a process that only knows the variable *name* cannot forge the capability.
pub const GATEWAY_ENV: &str = "BASEMIND_GATEWAY_TOKEN";

/// The tools withheld from the agent inside a protected workspace.
pub const WITHHELD_TOOLS: &[&str] = &["redact_text", "vault"];

/// True when `root` carries a `safe/` mirror.
///
/// Fails closed: any `safe` entry marks the workspace as protected, including a symlink
/// (which may point at the originals rather than at a mirror) — and so does an entry the
/// check cannot look at. A mirror inspection that ends in a permission or I/O error is not
/// an absence: pretending the workspace were ordinary would hand the agent the two tools
/// and the upstream resources precisely in the workspace that is carrying them.
pub fn is_protected(root: &Path) -> bool {
    mirror_marks_protected(std::fs::symlink_metadata(root.join(MIRROR_DIR)))
}

/// The decision isomorphic to the inspection result, so both branches carry tests.
fn mirror_marks_protected(inspection: Result<std::fs::Metadata, std::io::Error>) -> bool {
    match inspection {
        Ok(_) => true,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => true, // present, but uninspectable: fail closed
    }
}

/// True when this process is the gateway's own basemind process.
pub fn gateway_capable() -> bool {
    std::env::var(GATEWAY_ENV).is_ok_and(|value| !value.trim().is_empty())
}

/// True when `name` is a tool withheld from the agent inside a protected workspace.
pub fn is_withheld_tool(name: &str) -> bool {
    WITHHELD_TOOLS.contains(&name)
}

/// The refusal for a `memory` call whose modes reach the **shared** tier inside a protected
/// workspace, or `None` when the call may pass.
///
/// The shared tier is what the withheld **resource** `basemind://memory/{key}` serves, so the
/// tool cannot hand out the same content: its default `get`, `list`, `search`, `put`, `delete`
/// and `audit` modes all address the shared namespace (the visibility selector defaults to
/// `group`), and `mine`, `proposals`, `accept` and `reject` operate the shared governance
/// surface. Private `individual` memory (scoped to the calling agent, owner-segmented in the
/// store) is not exposed through that resource surface, so it stays available.
///
/// `mode` and `visibility` are read raw from the call arguments: this gate runs before the
/// handler parses them, and an unknown mode falls through (`false`) — the handler's own mode
/// parser rejects it.
pub fn memory_shared_refusal(args: Option<&serde_json::Map<String, serde_json::Value>>) -> Option<String> {
    let args = args?;
    let mode = args.get("mode").and_then(serde_json::Value::as_str)?;
    let visibility = args.get("visibility").and_then(serde_json::Value::as_str);
    let shared = visibility.is_none_or(|value| !value.trim().eq_ignore_ascii_case("individual"));
    let refused = match mode.trim().to_ascii_lowercase().as_str() {
        "documents" => false, // searches the workspace's own (masked) index, not shared memory
        "mine" | "proposals" | "accept" | "reject" => true,
        "put" | "get" | "list" | "search" | "delete" | "audit" => shared,
        _ => false,
    };
    refused.then(|| {
        format!(
            "`memory` on the shared tier is not available in a protected workspace: this workspace \
         carries a `{MIRROR_DIR}/` mirror, and the shared memory is not masked by it — the same \
         content the `basemind://memory/{{key}}` resource is refused for. Scope the call with \
         `visibility: \"individual\"` for the agent's own private entries.",
        )
    })
}

/// The message an agent gets when it reaches for a withheld tool. It names the reason and
/// the way out, so a refusal reads as a policy and not as a missing feature.
pub fn withheld_message(tool: &str) -> String {
    format!(
        "`{tool}` is not available in a protected workspace: this workspace carries a \
         `{MIRROR_DIR}/` mirror, and the token→original table belongs to the privacy gateway. \
         The agent sees tokens only."
    )
}

#[cfg(test)]
mod memory_gate_tests {
    use super::*;

    fn arguments(value: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        value.as_object().expect("object").clone()
    }

    #[test]
    fn the_default_visibility_is_the_shared_tier() {
        let refusal = memory_shared_refusal(Some(&arguments(serde_json::json!({
            "mode": "get", "key": "case/notes",
        }))));
        assert!(refusal.is_some());
    }

    #[test]
    fn every_mode_touching_shared_memory_is_refused() {
        for mode in ["put", "list", "search", "delete", "audit"] {
            let refusal = memory_shared_refusal(Some(&arguments(serde_json::json!({
                "mode": mode,
            }))));
            assert!(refusal.is_some(), "mode `{mode}` must be refused");
        }
        for mode in ["mine", "proposals", "accept", "reject"] {
            let refusal = memory_shared_refusal(Some(&arguments(serde_json::json!({ "mode": mode }))));
            assert!(refusal.is_some(), "mode `{mode}` must be refused");
        }
    }

    #[test]
    fn a_group_visibility_is_refused_regardless_of_spelling() {
        for spelling in ["group", "Group", "GROUP"] {
            let refusal = memory_shared_refusal(Some(&arguments(serde_json::json!({
                "mode": "search", "query": "show me", "visibility": spelling,
            }))));
            assert!(refusal.is_some(), "visibility `{spelling}` is shared");
        }
    }

    #[test]
    fn the_private_tier_passes_and_so_does_the_workspace_index() {
        for call in [
            serde_json::json!({ "mode": "get", "key": "mine", "visibility": "individual" }),
            serde_json::json!({ "mode": "put", "key": "mine", "value": "v", "visibility": "individual" }),
            serde_json::json!({ "mode": "list", "visibility": "INDIVIDUAL" }),
            serde_json::json!({ "mode": "search", "query": "q", "visibility": "individual" }),
            serde_json::json!({ "mode": "delete", "key": "mine", "visibility": "individual" }),
            serde_json::json!({ "mode": "audit", "visibility": "individual" }),
            serde_json::json!({ "mode": "documents", "query": "q" }),
        ] {
            let refusal = memory_shared_refusal(Some(&arguments(call)));
            assert!(refusal.is_none());
        }
    }

    #[test]
    fn everything_the_gate_cannot_judge_falls_through() {
        // No arguments, a missing mode, or an unknown mode: the gate must not guess — the
        // handler's own parser rejects what it cannot read.
        assert!(memory_shared_refusal(None).is_none());
        assert!(memory_shared_refusal(Some(&arguments(serde_json::json!({})))).is_none());
        assert!(memory_shared_refusal(Some(&arguments(serde_json::json!({ "mode": "bogus" })))).is_none());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_workspace_without_a_mirror_is_not_protected() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_protected(dir.path()));
    }

    #[test]
    fn a_mirror_the_check_cannot_look_at_is_still_protected() {
        // A confirmed absence (`NotFound`) is the only state that keeps the workspace
        // ordinary; a permission or I/O error turns the boundary on instead of off.
        assert!(mirror_marks_protected(Err(std::io::Error::from_raw_os_error(13)))); // EACCES
    }

    #[test]
    fn a_mirror_marks_the_workspace_protected() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(MIRROR_DIR)).unwrap();
        assert!(is_protected(dir.path()));
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_mirror_still_marks_the_workspace_protected() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("elsewhere.md"), "x").unwrap();
        std::os::unix::fs::symlink(dir.path(), dir.path().join(MIRROR_DIR)).unwrap();
        assert!(is_protected(dir.path()));
    }

    #[test]
    fn the_two_tools_are_the_withheld_ones() {
        assert!(is_withheld_tool("redact_text"));
        assert!(is_withheld_tool("vault"));
        assert!(!is_withheld_tool("code"));
        assert!(!is_withheld_tool("memory"));
    }

    #[test]
    fn the_refusal_names_the_tool_and_the_reason() {
        let message = withheld_message("vault");
        assert!(message.contains("`vault`"), "{message}");
        assert!(message.contains("protected workspace"), "{message}");
    }
}
