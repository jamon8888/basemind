//! Durable document-search generation sidecar and Lance schema migration helpers.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::{GENERATION_SUFFIX, LanceMeta};

pub(super) fn generation_path(dir: &Path) -> PathBuf {
    let name = dir.file_name().and_then(|name| name.to_str()).unwrap_or("lance");
    dir.with_file_name(format!("{name}{GENERATION_SUFFIX}"))
}

pub(super) fn read_generation(path: &Path) -> Result<u64> {
    match std::fs::read_to_string(path) {
        Ok(body) => body
            .trim()
            .parse::<u64>()
            .with_context(|| format!("parse document generation {}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(1),
        Err(error) => Err(error).with_context(|| format!("read document generation {}", path.display())),
    }
}

pub(super) fn bump_generation_file(path: &Path, current: u64) -> Result<u64> {
    let next = current.checked_add(1).unwrap_or(1);
    let tmp = path.with_extension("generation.tmp");
    std::fs::write(&tmp, next.to_string()).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("replace {}", path.display()))?;
    Ok(next)
}

pub(super) fn wipe_on_mismatch(dir: &Path, meta_path: &Path, expected: &LanceMeta) -> Result<bool> {
    if !meta_path.exists() {
        return Ok(false);
    }
    let bytes = std::fs::read(meta_path).with_context(|| format!("read {}", meta_path.display()))?;
    let actual: LanceMeta = serde_json::from_slice(&bytes).with_context(|| format!("parse {}", meta_path.display()))?;
    if actual == *expected {
        return Ok(false);
    }
    tracing::warn!(
        old_dim = actual.dim,
        new_dim = expected.dim,
        old_model = %actual.embedding_model,
        new_model = %expected.embedding_model,
        old_schema_ver = actual.schema_ver,
        new_schema_ver = expected.schema_ver,
        "lance store dim/model/schema mismatch — wiping {}",
        dir.display()
    );
    for entry in std::fs::read_dir(dir).with_context(|| format!("read_dir {}", dir.display()))? {
        let entry = entry.context("entry")?;
        let p = entry.path();
        if p.is_dir() {
            std::fs::remove_dir_all(&p).with_context(|| format!("remove {}", p.display()))?;
        } else {
            std::fs::remove_file(&p).with_context(|| format!("remove {}", p.display()))?;
        }
    }
    Ok(true)
}
