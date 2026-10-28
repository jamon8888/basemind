//! Public scope helpers for benchmark and compatibility tooling.

use std::path::Path;

use crate::git::Repo;

/// Return the scope used by a working-tree scan.
pub fn working_tree_scope(root: &Path) -> String {
    match Repo::discover(root) {
        Ok(repo) => crate::git::scope_key(&repo),
        Err(_) => format!("path:{}", root.display()),
    }
}
