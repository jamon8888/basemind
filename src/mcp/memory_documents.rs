//! Document-search cursor helpers and metadata filtering.

use rmcp::ErrorData as McpError;

use super::ServerState;
use super::types::{DocumentSearchHit, SearchDocumentsParams};

pub(super) const MAX_DOCUMENT_CURSOR_OFFSET: u64 = 10_000;

pub(super) fn document_query_fingerprint(
    params: &SearchDocumentsParams,
    scope: &str,
    lexical_only: bool,
    max_relaxations: u32,
    reranker_enabled: bool,
    reranker_top_k: usize,
    reranker_model: Option<&str>,
) -> u64 {
    let max_relaxations = max_relaxations.to_string();
    let reranker_top_k = reranker_top_k.to_string();
    let max_tokens = params.max_tokens.unwrap_or_default().to_string();
    let fields = [
        scope,
        if lexical_only { "lexical" } else { "vector" },
        max_relaxations.as_str(),
        if reranker_enabled {
            "reranker-on"
        } else {
            "reranker-off"
        },
        reranker_top_k.as_str(),
        params.query.as_str(),
        if params.mime_type.is_some() {
            "mime-set"
        } else {
            "mime-unset"
        },
        params.mime_type.as_deref().unwrap_or_default(),
        params.entity_category.as_deref().unwrap_or_default(),
        params.keywords_contains.as_deref().unwrap_or_default(),
        reranker_model.unwrap_or_default(),
        max_tokens.as_str(),
    ];
    let mut material = Vec::new();
    for field in fields {
        material.extend_from_slice(&(field.len() as u64).to_le_bytes());
        material.extend_from_slice(field.as_bytes());
    }
    let digest = blake3::hash(&material);
    u64::from_le_bytes(digest.as_bytes()[..8].try_into().expect("blake3 digest has 8 bytes"))
}

pub(super) fn document_cursor_offset(
    cursor: Option<&crate::mcp::cursor::Cursor>,
    generation: u64,
    query_fingerprint: u64,
) -> Result<(usize, bool), McpError> {
    let Some(cursor) = cursor else {
        return Ok((0, false));
    };
    let (offset, cursor_generation, cursor_fingerprint) = cursor.decode_documents()?;
    if cursor_generation != generation || cursor_fingerprint != query_fingerprint {
        return Ok((0, true));
    }
    if offset > MAX_DOCUMENT_CURSOR_OFFSET {
        return Err(McpError::invalid_params(
            format!("document cursor offset exceeds {MAX_DOCUMENT_CURSOR_OFFSET}"),
            None,
        ));
    }
    Ok((offset as usize, false))
}

/// Apply parent-document facet filters to the retrieved hits before reranking.
pub(super) async fn attach_doc_metadata(
    _state: &ServerState,
    hits: &mut Vec<DocumentSearchHit>,
    entity_category: Option<&str>,
    keywords_contains: Option<&str>,
) -> Result<(), McpError> {
    let cat_needle = entity_category.map(str::to_lowercase);
    let kw_needle = keywords_contains.map(str::to_lowercase);
    hits.retain_mut(|hit| {
        if let Some(needle) = cat_needle.as_deref()
            && !hit.entities.iter().any(|e| e.category.to_lowercase().contains(needle))
        {
            return false;
        }
        if let Some(needle) = kw_needle.as_deref()
            && !hit.keywords.iter().any(|k| k.text.to_lowercase().contains(needle))
        {
            return false;
        }
        true
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::cursor::Cursor;

    fn params() -> SearchDocumentsParams {
        SearchDocumentsParams {
            query: "alpha".to_string(),
            limit: Some(10),
            max_tokens: None,
            format: None,
            mime_type: None,
            doc_type: None,
            section: None,
            jurisdiction: None,
            doc_date: None,
            scope: Some("repo".to_string()),
            entity_category: None,
            keywords_contains: None,
            cursor: None,
            overrides: Default::default(),
        }
    }

    #[test]
    fn cursor_generation_and_query_mismatch_invalidates() {
        let params = params();
        let fingerprint = document_query_fingerprint(&params, "repo", false, 0, false, 0, None);
        let cursor = Cursor::encode_documents(10, 4, fingerprint);
        assert_eq!(
            document_cursor_offset(Some(&cursor), 4, fingerprint).unwrap(),
            (10, false)
        );
        assert_eq!(
            document_cursor_offset(Some(&cursor), 5, fingerprint).unwrap(),
            (0, true)
        );
        assert_eq!(
            document_cursor_offset(Some(&cursor), 4, fingerprint.wrapping_add(1)).unwrap(),
            (0, true)
        );
        let oversized = Cursor::encode_documents(MAX_DOCUMENT_CURSOR_OFFSET + 1, 4, fingerprint);
        assert!(document_cursor_offset(Some(&oversized), 4, fingerprint).is_err());
        let mut newline_params = params;
        newline_params.query = "alpha\nrepo".to_string();
        let first = document_query_fingerprint(&newline_params, "repo", false, 0, false, 0, None);
        let mut shifted = newline_params;
        shifted.query = "alpha".to_string();
        shifted.mime_type = Some("repo\n".to_string());
        let second = document_query_fingerprint(&shifted, "repo", false, 0, false, 0, None);
        assert_ne!(first, second);
    }
}
