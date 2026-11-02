//! Public graph API surface on [`LanceStore`] (spec 0016).
//!
//! Split from `mod.rs` to honor the module-size cap. These impl blocks expose the
//! graphrag read surface to external callers (hacienda-backend's MCP `graph entities`
//! / `graph rag` tools) without handing out the store's private connection/runtime:
//! the store owns the LanceDB connection and the blocking runtime, and every method
//! here block_ons that runtime — so callers off the async path invoke these directly,
//! and async callers wrap them in `spawn_blocking`.

use anyhow::{Context, Result, anyhow};

use super::{graphrag, CypherResult, DocumentHit, LanceStore};

impl LanceStore {
    /// Execute a read-only Cypher query over this store's entity graph (spec 0016).
    ///
    /// Public wrapper around [`graphrag::execute_cypher`]. Only `MATCH`, `RETURN`,
    /// `WHERE`, `LIMIT`, `ORDER BY`, `WITH`, `UNION` pass lance-graph's parser;
    /// `CREATE`/`DELETE`/`SET`/`MERGE` are rejected, so the surface is read-only.
    #[cfg(feature = "graphrag")]
    pub fn execute_cypher(&self, scope: &str, cypher: &str, max_rows: usize) -> Result<CypherResult> {
        graphrag::execute_cypher(&self.inner.connection, self.inner.rt(), scope, cypher, max_rows)
    }

    /// GraphRAG retrieval over this store's documents + entity co-occurrence graph (spec 0016).
    ///
    /// One sync call encapsulating the whole `graph rag` flow: vector lane (top-k
    /// document chunks for `query_vec`), entity seeds from the hit documents, bounded
    /// BFS expansion over the co-occurrence edges (capped at 2 hops / 200 nodes),
    /// chunk fetch for graph-only documents, and score fusion. Returns the fused hits
    /// plus the truncation flag at `max_results`.
    #[cfg(feature = "graphrag")]
    pub fn graph_rag_search(
        &self,
        scope: &str,
        query_vec: Vec<f32>,
        hops: u8,
        max_results: usize,
    ) -> Result<(Vec<graphrag::GraphRagHit>, bool)> {
        use std::collections::{HashMap, HashSet};

        use graphrag::{expand_entity_graph, fuse_results, load_edge_endpoints, load_entity_refs};

        if query_vec.len() != usize::from(self.inner.dim) {
            return Err(anyhow!(
                "query vector dim {} does not match store dim {}",
                query_vec.len(),
                self.inner.dim
            ));
        }
        let hops = hops.min(2);
        let k = max_results.min(50);

        // 1. Vector lane: top-k document chunks.
        let vector_hits = self.search_documents(scope, query_vec, k, None)?;

        // 2. Load the graph batches once; seeds = entities of the vector-hit docs.
        let entities_batch = self
            .inner
            .rt()
            .block_on(graphrag::load_graph_entities_batch(&self.inner.connection, scope))
            .context("load graph_entities")?;
        let edges_batch = self
            .inner
            .rt()
            .block_on(graphrag::load_graph_edges_batch(&self.inner.connection, scope))
            .context("load graph_edges")?;
        let entities = load_entity_refs(&entities_batch)?;
        let (edge_src, edge_dst) = load_edge_endpoints(&edges_batch)?;

        let hit_paths: HashSet<String> = vector_hits.iter().map(|h| h.path.clone()).collect();
        let mut seeds: Vec<String> = Vec::new();
        let mut seed_seen = HashSet::new();
        for entity in &entities {
            if hit_paths.contains(&entity.document_path) && seed_seen.insert(entity.entity_id.clone()) {
                seeds.push(entity.entity_id.clone());
            }
        }

        // 3. Expand over the co-occurrence edges (bounded).
        let expanded = expand_entity_graph(&edge_src, &edge_dst, &seeds, hops, 200);
        let expanded_ids: HashSet<&str> = expanded.iter().map(|e| e.entity_id.as_str()).collect();
        let hop_of: HashMap<&str, u8> = expanded.iter().map(|e| (e.entity_id.as_str(), e.hop)).collect();
        let chain_of: HashMap<&str, Vec<String>> = expanded
            .iter()
            .map(|e| (e.entity_id.as_str(), e.chain.clone()))
            .collect();

        // 4a. Documents reachable through the graph but absent from the vector lane.
        let mut graph_doc_hop: HashMap<String, u8> = HashMap::new();
        for entity in &entities {
            if !expanded_ids.contains(entity.entity_id.as_str()) {
                continue;
            }
            let hop = hop_of.get(entity.entity_id.as_str()).copied().unwrap_or(0);
            let entry = graph_doc_hop.entry(entity.document_path.clone()).or_insert(hop);
            if hop < *entry {
                *entry = hop;
            }
        }
        let fresh_paths: Vec<String> = graph_doc_hop
            .keys()
            .filter(|p| !hit_paths.contains(*p))
            .cloned()
            .collect();
        let fresh_chunks = if fresh_paths.is_empty() {
            Vec::new()
        } else {
            self.document_chunks_for_paths(scope, &fresh_paths, 40)?
        };

        // 4b. Fuse.
        let graph_chunks: Vec<(DocumentHit, u8, Vec<String>)> = fresh_chunks
            .into_iter()
            .map(|hit| {
                let hop = graph_doc_hop.get(&hit.path).copied().unwrap_or(1).max(1);
                let chain = entities
                    .iter()
                    .find(|e| e.document_path == hit.path)
                    .and_then(|e| chain_of.get(e.entity_id.as_str()))
                    .cloned()
                    .unwrap_or_default();
                (hit, hop, chain)
            })
            .collect();
        Ok(fuse_results(vector_hits, graph_chunks, max_results))
    }
}
