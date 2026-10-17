//! Top-level `scan` / `rescan` argument groups for the CLI dispatch (`enum Cmd` stays in
//! `main.rs`); split out to respect the 1000-line module cap (`tests/max_lines.rs`).

use basemind::config::DocumentsCliOverrides;

#[derive(clap::Args, Debug)]
pub struct ScanArgs {
    /// Index the git staging area instead of the working tree. Used by the
    /// pre-commit hook so the cache reflects what's about to be committed.
    /// Mutually exclusive with --rev.
    #[arg(long, conflicts_with = "rev")]
    pub staged: bool,
    /// Index the tree at the given revision (HEAD, branch name, sha, HEAD~3).
    /// Written as the `rev-<sha7>` view in the machine-global workspace cache, separate from the
    /// working-tree view.
    #[arg(long, value_name = "REV")]
    pub rev: Option<String>,
    /// Skip building the git-history index after the scan (overrides config). The history tools
    /// then fall back to the live walk. Equivalent to `BASEMIND_GH_INDEX=0`.
    #[arg(long)]
    pub no_git_history: bool,
    /// Wipe and fully rebuild the git-history index instead of incrementally syncing it. Use after
    /// a history rewrite if revalidation didn't already trigger a rebuild.
    #[arg(long)]
    pub rebuild_git_history: bool,
    /// Document-tier overrides. Every flag in this group corresponds to a
    /// `[documents.…]` TOML key and a `BASEMIND_DOCUMENTS_…` env var.
    #[command(flatten)]
    pub documents: DocumentsCliOverrides,
}

#[derive(clap::Args, Debug)]
pub struct RescanArgs {
    /// Repo-relative paths to re-index incrementally. When omitted (or with `--full`),
    /// the entire working tree is re-indexed. Paths are forward-slash with no leading `/`.
    #[arg(value_name = "PATH")]
    pub paths: Vec<String>,
    /// Force a full working-tree re-index even when paths are supplied. Use to rebuild a
    /// stale or empty index from scratch.
    #[arg(long)]
    pub full: bool,
    /// Skip building the git-history index after the rescan (overrides config).
    #[arg(long)]
    pub no_git_history: bool,
    /// Wipe and fully rebuild the git-history index instead of incrementally syncing it.
    #[arg(long)]
    pub rebuild_git_history: bool,
}
