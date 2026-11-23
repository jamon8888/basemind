//! Contract of the two supported Basemind profiles.
//!
//! These are build-time contracts, not runtime switches. `full` deliberately keeps its historical
//! composition — documents, memory, crawl, comms, shells, code intelligence, semantic code search
//! and Candle NER. The confidential `hacienda-legal` profile keeps documents, memory, Candle NER
//! and GraphRAG, and withholds code, comms and crawl on every surface. Each test is gated to the
//! profile it guards so the two cannot silently converge.
//!
//! The feature assertions live in `const` blocks: `cfg!` expands to a compile-time literal, so a
//! plain runtime `assert!` is a constant assertion that clippy rejects under `-D warnings`. The
//! explicit const block keeps the same compile-time contract and makes that intent visible.

#[cfg(any(feature = "full", feature = "hacienda-legal"))]
use basemind::mcp::mode::domain_modes;

// ---------------------------------------------------------------------------
// `full`: the complete profile
// ---------------------------------------------------------------------------

#[cfg(feature = "full")]
#[test]
fn full_keeps_the_complete_capability_set() {
    const { assert!(cfg!(feature = "documents"), "full must compile documents") };
    const { assert!(cfg!(feature = "memory"), "full must compile memory") };
    const { assert!(cfg!(feature = "ner-candle"), "full must compile Candle NER") };
    const { assert!(cfg!(feature = "crawl"), "full must compile crawl") };
    const { assert!(cfg!(feature = "comms"), "full must compile comms") };
    const { assert!(cfg!(feature = "shells"), "full must compile shells") };
    const {
        assert!(
            cfg!(feature = "code-intel"),
            "full must compile precise code intelligence"
        )
    };
    const { assert!(cfg!(feature = "code-search"), "full must compile semantic code search") };
    const {
        assert!(
            !cfg!(feature = "hacienda-legal"),
            "the two profiles are mutually exclusive"
        )
    };
    // GraphRAG belongs to the confidential legal profile; `full` stays on the historical build.
    const { assert!(!cfg!(feature = "graphrag"), "full must not pull in GraphRAG") };
}

#[cfg(feature = "full")]
#[test]
fn full_advertises_code_comms_crawl_and_privacy_domains() {
    let domains: Vec<&str> = domain_modes().iter().map(|(domain, _)| *domain).collect();
    for expected in [
        "code",
        "graph",
        "git",
        "memory",
        "admin",
        "vault",
        "web",
        "agents",
        "workspace",
        "shell",
    ] {
        assert!(
            domains.contains(&expected),
            "{expected} must be advertised by full: {domains:?}"
        );
    }
}

#[cfg(feature = "full")]
#[test]
fn full_cli_still_exposes_code_and_web() {
    let exe = env!("CARGO_BIN_EXE_basemind");
    for command in ["code", "web"] {
        let output = std::process::Command::new(exe)
            .args([command, "-h"])
            .output()
            .expect("spawn basemind help");
        assert!(output.status.success(), "`basemind {command}` must exist in full");
    }
}

// ---------------------------------------------------------------------------
// `hacienda-legal`: the confidential professional profile
// ---------------------------------------------------------------------------

#[cfg(feature = "hacienda-legal")]
#[test]
fn legal_compiles_only_the_confidential_feature_set() {
    const { assert!(cfg!(feature = "documents"), "legal must compile documents") };
    const { assert!(cfg!(feature = "memory"), "legal must compile memory") };
    const { assert!(cfg!(feature = "ner-candle"), "legal must compile Candle NER") };
    const { assert!(cfg!(feature = "graphrag"), "legal must compile GraphRAG") };
    const {
        assert!(
            !cfg!(feature = "crawl"),
            "no crawl transport may be compiled into the confidential profile"
        )
    };
    const {
        assert!(
            !cfg!(feature = "comms"),
            "no comms broker/daemon may be compiled into the confidential profile"
        )
    };
    const { assert!(!cfg!(feature = "shells"), "legal has no detached shells") };
    const { assert!(!cfg!(feature = "code-intel"), "legal has no code-intelligence engines") };
    const { assert!(!cfg!(feature = "code-search"), "legal has no semantic code search") };
    const { assert!(!cfg!(feature = "full"), "the two profiles are mutually exclusive") };
}

#[cfg(feature = "hacienda-legal")]
#[test]
fn legal_advertises_documents_memory_and_graphrag_only() {
    let domains: Vec<&str> = domain_modes().iter().map(|(domain, _)| *domain).collect();
    for expected in ["graph", "git", "memory", "admin", "vault"] {
        assert!(
            domains.contains(&expected),
            "{expected} must be advertised by hacienda-legal: {domains:?}"
        );
    }
    for withheld in ["code", "web", "agents", "workspace", "shell"] {
        assert!(
            !domains.contains(&withheld),
            "{withheld} must be withheld by hacienda-legal: {domains:?}"
        );
    }
    let graph_modes = domain_modes()
        .iter()
        .find(|(domain, _)| *domain == "graph")
        .map(|(_, modes)| *modes)
        .expect("graph domain");
    for mode in ["entities", "rag"] {
        assert!(
            graph_modes.contains(&mode),
            "graph mode {mode} must be live in hacienda-legal: {graph_modes:?}"
        );
    }
}
