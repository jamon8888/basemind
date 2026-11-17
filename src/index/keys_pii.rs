//! Byte-level key encoding for the `pii_lineage` Fjall keyspace.
//!
//! Each PII entity is addressed by a `(scope, file_id, entity_id)` composite key
//! (spec §8: "composite key `(scope, file_id, entity_id)`"), where `file_id` is the
//! SHA-256 hex of the source file. Components are u16-length-prefixed with NUL
//! separators, mirroring [`crate::index::keys_governance`], so prefix scans stay
//! unambiguous: [`pii_lineage_file_prefix`] lists one file's entities,
//! [`pii_lineage_scope_prefix`] bounds one workspace.

use super::keys::{read_len_prefixed, write_len_prefixed};

/// Encode a full entity key: `len(scope)‖scope‖0x00‖len(file_id)‖file_id‖0x00‖len(entity_id)‖entity_id`.
/// Returns `None` when any component exceeds the 64 KiB u16 ceiling.
pub fn pii_lineage_key(scope: &str, file_id: &str, entity_id: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(2 + scope.len() + 1 + 2 + file_id.len() + 1 + 2 + entity_id.len());
    write_len_prefixed(&mut out, scope.as_bytes())?;
    out.push(0u8);
    write_len_prefixed(&mut out, file_id.as_bytes())?;
    out.push(0u8);
    write_len_prefixed(&mut out, entity_id.as_bytes())?;
    Some(out)
}

/// Prefix bytes for every entity in one `(scope, file_id)` — everything up to and
/// including the NUL after `file_id`. Feed to `keyspace.prefix(..)`.
pub fn pii_lineage_file_prefix(scope: &str, file_id: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(2 + scope.len() + 1 + 2 + file_id.len() + 1);
    write_len_prefixed(&mut out, scope.as_bytes())?;
    out.push(0u8);
    write_len_prefixed(&mut out, file_id.as_bytes())?;
    out.push(0u8);
    Some(out)
}

/// Prefix bytes for every entity in one `scope`. Length-prefixing keeps scopes
/// from spilling into each other (a longer scope encodes a different length).
pub fn pii_lineage_scope_prefix(scope: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(2 + scope.len() + 1);
    write_len_prefixed(&mut out, scope.as_bytes())?;
    out.push(0u8);
    Some(out)
}

/// Decode a [`pii_lineage_key`] back into `(scope, file_id, entity_id)`.
/// Returns `None` on truncated input or a missing NUL separator.
pub fn parse_pii_lineage_key(buf: &[u8]) -> Option<(String, String, String)> {
    let mut cursor = 0;
    let scope = String::from_utf8(read_len_prefixed(buf, &mut cursor)?).ok()?;
    if buf.get(cursor) != Some(&0u8) {
        return None;
    }
    cursor += 1;
    let file_id = String::from_utf8(read_len_prefixed(buf, &mut cursor)?).ok()?;
    if buf.get(cursor) != Some(&0u8) {
        return None;
    }
    cursor += 1;
    let entity_id = String::from_utf8(read_len_prefixed(buf, &mut cursor)?).ok()?;
    if cursor != buf.len() {
        return None;
    }
    Some((scope, file_id, entity_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_roundtrips_components() {
        let key = pii_lineage_key("ws-1", "ab".repeat(32).as_str(), "e-9").unwrap();
        let (scope, file_id, entity_id) = parse_pii_lineage_key(&key).unwrap();
        assert_eq!(scope, "ws-1");
        assert_eq!(file_id, "ab".repeat(32));
        assert_eq!(entity_id, "e-9");
    }

    #[test]
    fn file_prefix_bounds_one_file() {
        let prefix = pii_lineage_file_prefix("ws", "f1").unwrap();
        let same = pii_lineage_key("ws", "f1", "e1").unwrap();
        let other_file = pii_lineage_key("ws", "f2", "e1").unwrap();
        let other_scope = pii_lineage_key("ws2", "f1", "e1").unwrap();
        assert!(same.starts_with(&prefix));
        assert!(!other_file.starts_with(&prefix));
        assert!(!other_scope.starts_with(&prefix));
    }

    #[test]
    fn scope_prefix_bounds_one_scope() {
        let prefix = pii_lineage_scope_prefix("ws").unwrap();
        assert!(pii_lineage_key("ws", "f", "e").unwrap().starts_with(&prefix));
        assert!(!pii_lineage_key("ws2", "f", "e").unwrap().starts_with(&prefix));
    }

    #[test]
    fn parse_rejects_truncation() {
        let key = pii_lineage_key("ws", "f1", "e1").unwrap();
        assert!(parse_pii_lineage_key(&key[..key.len() - 2]).is_none());
        assert!(parse_pii_lineage_key(b"garbage").is_none());
    }
}
