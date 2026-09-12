//! Post-extraction translation: redaction findings plus chunk boundaries in,
//! lineage records plus volume counters out.
//!
//! The single scan-lane write seam behind the GDPR audit trail. Report offsets
//! are byte-exact in the original content; chunk indexes derive from the
//! persisted chunk spans. The report carries no confidence, so translated
//! entities record their category threshold as implied confidence ("met the
//! bar, strength unknown") with detector `xberg-redaction-v1`. Raw bytes are
//! dropped by the engine, so the value hash covers the replacement token.
//! Entity IDs are deterministic (`file_id:category:start:end`) so rescans
//! upsert instead of duplicating.

use std::collections::HashMap;

use super::{EntityLocation, PiiEntity, confidence_threshold, risk_for_label};
use crate::hashing::{hash_bytes, hex};

/// One redaction finding in tracker-neutral form (byte offsets, label, token).
pub struct FindingInput {
    pub start: u32,
    pub end: u32,
    pub category: String,
    pub token: String,
}

/// One persisted chunk span for offset-to-chunk mapping.
pub struct ChunkSpan {
    pub byte_start: u32,
    pub byte_end: u32,
}

/// Per-scan detection counters accompanying a translation.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct TranslationStats {
    pub total: u32,
    pub by_category: HashMap<String, u32>,
    /// Findings with no containing chunk (offsets past the chunk map).
    pub unmapped: u32,
}

pub fn translate_findings(
    _scope: &str,
    file_id: &str,
    findings: &[FindingInput],
    chunks: &[ChunkSpan],
    detected_at: i64,
    processed_by: &str,
) -> (Vec<PiiEntity>, TranslationStats) {
    let mut entities = Vec::with_capacity(findings.len());
    let mut stats = TranslationStats::default();
    let num_chunks = chunks.len();
    let last_idx = num_chunks.saturating_sub(1);

    for f in findings {
        stats.total += 1;
        *stats.by_category.entry(f.category.clone()).or_insert(0) += 1;

        let start = f.start as i64;
        let mut chunk_idx = last_idx;
        let mut unmapped = false;

        if num_chunks > 0 {
            let mut found = false;
            for (i, ch) in chunks.iter().enumerate() {
                let ch_start = ch.byte_start as i64;
                let ch_end = ch.byte_end as i64;
                if start >= ch_start && start < ch_end {
                    chunk_idx = i;
                    found = true;
                    break;
                }
            }
            if !found {
                unmapped = true;
            }
        } else {
            unmapped = true;
        }

        if unmapped {
            stats.unmapped += 1;
        }

        let entity_id = format!("{file_id}:{}:{}:{}", f.category, f.start, f.end);
        let value_hash = hex(&hash_bytes(f.token.as_bytes()));
        let confidence = confidence_threshold(&f.category);

        let location = EntityLocation {
            file_id: file_id.to_string(),
            chunk_index: chunk_idx as i32,
            char_start: -1,
            char_end: -1,
            byte_start: Some(f.start as i32),
            byte_end: Some(f.end as i32),
            page_number: None,
            context: String::new(),
        };

        let entity = PiiEntity {
            entity_id,
            category: f.category.clone(),
            subcategory: None,
            value_hash,
            confidence,
            detector_version: "xberg-redaction-v1".to_string(),
            locations: vec![location],
            detected_at,
            processed_by: processed_by.to_string(),
            legal_basis: None,
            retention_until: None,
            risk_level: risk_for_label(&f.category),
            lineage: Vec::new(),
        };

        entities.push(entity);
    }

    (entities, stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunks() -> Vec<ChunkSpan> {
        vec![
            ChunkSpan {
                byte_start: 0,
                byte_end: 100,
            },
            ChunkSpan {
                byte_start: 100,
                byte_end: 200,
            },
        ]
    }

    fn findings() -> Vec<FindingInput> {
        vec![
            FindingInput {
                start: 10,
                end: 32,
                category: "iban".into(),
                token: "[IBAN_1]".into(),
            },
            FindingInput {
                start: 150,
                end: 165,
                category: "email".into(),
                token: "[EMAIL_1]".into(),
            },
        ]
    }

    #[test]
    fn golden_translation() {
        let (entities, stats) = translate_findings("ws", "f1", &findings(), &chunks(), 1786051200000000, "t");
        assert_eq!(entities.len(), 2);
        let iban = &entities[0];
        assert_eq!(iban.entity_id, "f1:iban:10:32");
        assert_eq!(iban.category, "iban");
        assert_eq!(iban.value_hash, hex(&hash_bytes(b"[IBAN_1]")));
        assert_eq!(iban.confidence, confidence_threshold("iban"));
        assert_eq!(iban.detector_version, "xberg-redaction-v1");
        assert_eq!(iban.detected_at, 1786051200000000);
        assert_eq!(iban.processed_by, "t");
        assert_eq!(iban.locations.len(), 1);
        let loc = &iban.locations[0];
        assert_eq!(loc.file_id, "f1");
        assert_eq!(loc.chunk_index, 0);
        assert_eq!(loc.char_start, -1);
        assert_eq!(loc.byte_start, Some(10));
        assert_eq!(loc.byte_end, Some(32));
        assert_eq!(entities[1].locations[0].chunk_index, 1);
        assert_eq!(stats.total, 2);
        assert_eq!(stats.by_category.get("iban"), Some(&1));
        assert_eq!(stats.by_category.get("email"), Some(&1));
        assert_eq!(stats.unmapped, 0);
    }

    #[test]
    fn finding_past_chunks_maps_to_last_with_unmapped_count() {
        let beyond = vec![FindingInput {
            start: 500,
            end: 510,
            category: "phone".into(),
            token: "[PHONE_1]".into(),
        }];
        let (entities, stats) = translate_findings("ws", "f1", &beyond, &chunks(), 0, "t");
        assert_eq!(entities[0].locations[0].chunk_index, 1);
        assert_eq!(stats.unmapped, 1);
    }

    #[test]
    fn empty_report_yields_nothing() {
        let (entities, stats) = translate_findings("ws", "f1", &[], &chunks(), 0, "t");
        assert!(entities.is_empty());
        assert_eq!(stats, TranslationStats::default());
    }

    #[test]
    fn rescan_is_idempotent() {
        let (first, _) = translate_findings("ws", "f1", &findings(), &chunks(), 1, "t");
        let (second, _) = translate_findings("ws", "f1", &findings(), &chunks(), 2, "t");
        let ids1: Vec<_> = first.iter().map(|e| &e.entity_id).collect();
        let ids2: Vec<_> = second.iter().map(|e| &e.entity_id).collect();
        assert_eq!(ids1, ids2);
    }
}
