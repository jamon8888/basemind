//! CRUD over the `pii_lineage` Fjall keyspace: `(scope, file_id, entity_id)` →
//! msgpack [`PiiEntity`](crate::pii::PiiEntity).
//!
//! GDPR Article 30 audit trail. The scan lane writes one record per detected entity
//! ([`put_entity`]); erasure requests soft-erase via [`erase_entity`] (value hash
//! dropped, audit metadata preserved — see [`PiiEntity::soft_erase`](crate::pii::PiiEntity::soft_erase)).
//! Keys follow [`crate::index::keys_pii`]; values use named msgpack like the memory tier.

use fjall::Keyspace;

use super::IndexDb;
use super::keys::{pii_lineage_file_prefix, pii_lineage_key};
use super::{IndexError, open_keyspace};
use crate::pii::PiiEntity;

/// Open (or create) the standalone `pii_lineage` partition. Used by tests and by
/// future non-index hosts; the daemon path reads it off [`IndexDb::pii_lineage`].
pub fn open_partition(db: &fjall::Database) -> Result<Keyspace, IndexError> {
    open_keyspace(db, "pii_lineage")
}

fn encode_key(scope: &str, file_id: &str, entity_id: &str) -> Result<Vec<u8>, IndexError> {
    pii_lineage_key(scope, file_id, entity_id).ok_or(IndexError::KeyTooLong)
}

/// Store (or overwrite) one entity record.
pub fn put_entity(idx: &IndexDb, scope: &str, file_id: &str, entity: &PiiEntity) -> Result<(), IndexError> {
    let key = encode_key(scope, file_id, &entity.entity_id)?;
    let bytes = rmp_serde::to_vec_named(entity)?;
    idx.pii_lineage.insert(key, bytes)?;
    Ok(())
}

/// Fetch one entity record; `None` when absent.
pub fn get_entity(idx: &IndexDb, scope: &str, file_id: &str, entity_id: &str) -> Result<Option<PiiEntity>, IndexError> {
    let key = encode_key(scope, file_id, entity_id)?;
    let Some(bytes) = idx.pii_lineage.get(key)? else {
        return Ok(None);
    };
    Ok(Some(rmp_serde::from_slice(&bytes)?))
}

/// Every entity record filed under one `(scope, file_id)`, in key order.
/// Undecodable rows are skipped so one corrupt value can't fail an audit listing.
pub fn list_file_entities(idx: &IndexDb, scope: &str, file_id: &str) -> Result<Vec<PiiEntity>, IndexError> {
    let prefix = pii_lineage_file_prefix(scope, file_id).ok_or(IndexError::KeyTooLong)?;
    let mut out = Vec::new();
    for guard in idx.pii_lineage.prefix(prefix) {
        let (_k, v) = guard.into_inner()?;
        if let Ok(entity) = rmp_serde::from_slice::<PiiEntity>(&v) {
            out.push(entity);
        }
    }
    Ok(out)
}

/// Right-to-erasure soft erase (spec §9 step 3): rewrite the record with the value
/// hash dropped, preserving category/locations/detected_at. Returns `false` when
/// no record exists (nothing to erase); hard-deletes nothing.
pub fn erase_entity(idx: &IndexDb, scope: &str, file_id: &str, entity_id: &str) -> Result<bool, IndexError> {
    let Some(mut entity) = get_entity(idx, scope, file_id, entity_id)? else {
        return Ok(false);
    };
    entity.soft_erase();
    put_entity(idx, scope, file_id, &entity)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pii::{EntityLocation, RiskLevel};

    fn fresh_db() -> (tempfile::TempDir, IndexDb) {
        let dir = tempfile::tempdir().unwrap();
        let db = IndexDb::open(dir.path()).unwrap();
        (dir, db)
    }

    fn entity(id: &str, file_id: &str) -> PiiEntity {
        PiiEntity {
            entity_id: id.into(),
            category: "iban".into(),
            subcategory: None,
            value_hash: "abc123".into(),
            confidence: 0.97,
            detector_version: "rule-iban-v1".into(),
            locations: vec![EntityLocation {
                file_id: file_id.into(),
                chunk_index: 0,
                char_start: -1,
                char_end: -1,
                byte_start: Some(10),
                byte_end: Some(32),
                page_number: None,
                context: "…IBAN DE89…".into(),
            }],
            detected_at: 1786051200000000,
            processed_by: "test".into(),
            legal_basis: None,
            retention_until: None,
            risk_level: RiskLevel::High,
            lineage: vec![],
        }
    }

    #[test]
    fn put_get_roundtrip() {
        let (_dir, db) = fresh_db();
        put_entity(&db, "ws", "f1", &entity("e1", "f1")).unwrap();
        let got = get_entity(&db, "ws", "f1", "e1").unwrap().expect("stored");
        assert_eq!(got.entity_id, "e1");
        assert_eq!(got.value_hash, "abc123");
        assert!(get_entity(&db, "ws", "f1", "missing").unwrap().is_none());
        assert!(get_entity(&db, "ws", "other-file", "e1").unwrap().is_none());
    }

    #[test]
    fn list_file_returns_only_that_file() {
        let (_dir, db) = fresh_db();
        put_entity(&db, "ws", "f1", &entity("e1", "f1")).unwrap();
        put_entity(&db, "ws", "f1", &entity("e2", "f1")).unwrap();
        put_entity(&db, "ws", "f2", &entity("e3", "f2")).unwrap();
        let listed = list_file_entities(&db, "ws", "f1").unwrap();
        let ids: Vec<_> = listed.iter().map(|e| e.entity_id.as_str()).collect();
        assert_eq!(ids, vec!["e1", "e2"]);
        assert!(list_file_entities(&db, "ws", "empty").unwrap().is_empty());
    }

    #[test]
    fn get_entity_propagates_decode_error() {
        use crate::index::keys::pii_lineage_key;
        let (_dir, db) = fresh_db();
        let key = pii_lineage_key("ws", "f1", "corrupt").unwrap();
        db.pii_lineage.insert(key, b"not-msgpack".as_slice()).unwrap();
        assert!(matches!(
            get_entity(&db, "ws", "f1", "corrupt"),
            Err(IndexError::Decode(_))
        ));
        assert!(erase_entity(&db, "ws", "f1", "corrupt").is_err());
    }

    #[test]
    fn erase_preserves_audit_drops_hash() {
        let (_dir, db) = fresh_db();
        put_entity(&db, "ws", "f1", &entity("e1", "f1")).unwrap();
        assert!(erase_entity(&db, "ws", "f1", "e1").unwrap());
        let got = get_entity(&db, "ws", "f1", "e1").unwrap().expect("record preserved");
        assert!(got.is_erased());
        assert_eq!(got.category, "iban");
        assert_eq!(got.locations.len(), 1);
        assert!(got.locations.iter().all(|l| l.context.is_empty()));
        assert!(!erase_entity(&db, "ws", "f1", "missing").unwrap());
    }
}
