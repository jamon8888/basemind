use super::*;

#[test]
fn test_normalize_literal() {
    assert_eq!(normalize_literal("John Smith"), "john smith");
    assert_eq!(normalize_literal("JOHN  SMITH"), "john smith");
    assert_eq!(normalize_literal("john-smith"), "john smith");
    assert_eq!(normalize_literal("John.Smith"), "john smith");
    assert_eq!(normalize_literal("Müller-Straße"), "müller straße");
}

#[test]
fn test_entity_key_dedup() {
    let mut map = HashMap::new();
    let k1 = EntityNodeKey::ClearLiteral {
        label: "person".into(),
        normalized: "john smith".into(),
    };
    let k2 = EntityNodeKey::ClearLiteral {
        label: "person".into(),
        normalized: "john smith".into(),
    };
    map.insert(k1, 1);
    map.insert(k2, 2);
    assert_eq!(map.len(), 1);
}

#[test]
fn gateway_token_class_is_authoritative_for_graph_label() {
    let entity = DocEntitySidecar {
        label: "person".into(),
        literal: "<4b31c7ee:Email_1>".into(),
        is_token: false,
        is_gateway_token: true,
        gateway_token: Some("<4b31c7ee:Email_1>".into()),
        start: 0,
        end: 23,
        chunk_idx: 0,
        doc_path: "a.txt".into(),
    };
    assert_eq!(graph_label(&entity), "email");
}

#[test]
fn gateway_token_label_variance_deduplicates_to_one_node() {
    let entities = vec![
        DocEntitySidecar {
            label: "email".into(),
            literal: "<4b31c7ee:Email_1>".into(),
            is_token: false,
            is_gateway_token: true,
            gateway_token: Some("<4b31c7ee:Email_1>".into()),
            start: 0,
            end: 23,
            chunk_idx: 0,
            doc_path: "a.txt".into(),
        },
        DocEntitySidecar {
            label: "person".into(),
            literal: "<4b31c7ee:Email_1>".into(),
            is_token: false,
            is_gateway_token: true,
            gateway_token: Some("<4b31c7ee:Email_1>".into()),
            start: 24,
            end: 47,
            chunk_idx: 1,
            doc_path: "b.txt".into(),
        },
    ];
    let config = GraphragConfig {
        enabled: true,
        labels: vec!["email".into()],
        ..Default::default()
    };
    let (nodes, _) = build_graph_from_entities("scope", &entities, &config).unwrap();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].mention_count, 2);
    assert_eq!(nodes[0].label, "email");
}

/// The GraphRAG graph lane is operational end to end without downloading a model: scan-time
/// entities become `graph_entities` / `graph_edges` rows, the rows are read back from Lance,
/// and the entity IDs join across the edge endpoints so BFS expansion reaches co-occurring
/// entities. This is the exact pipeline `graph entities` and `graph rag` use at runtime.
#[test]
fn graphrag_lance_graph_lanes_round_trip_and_expand() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = super::super::LanceStore::open(dir.path(), 4, "graphrag-unit").expect("open lance store");
    let config = GraphragConfig {
        enabled: true,
        labels: vec!["person".into()],
        ..Default::default()
    };
    let sidecar = |literal: &str, chunk_idx: u32, doc_path: &str| DocEntitySidecar {
        label: "person".into(),
        literal: literal.into(),
        is_token: false,
        is_gateway_token: false,
        gateway_token: None,
        start: 0,
        end: literal.len() as u32,
        chunk_idx,
        doc_path: doc_path.into(),
    };
    let entities = vec![
        sidecar("Ada Lovelace", 0, "matter/notes.md"),
        sidecar("Charles Babbage", 0, "matter/notes.md"),
        sidecar("Ada Lovelace", 1, "matter/notes.md"),
    ];
    let (nodes, edges) = build_graph_from_entities("scope-a", &entities, &config).expect("build graph");
    assert_eq!(nodes.len(), 2, "two distinct people should become two nodes");
    assert!(!edges.is_empty(), "two people in one chunk must co-occur");
    assert!(
        nodes.iter().all(|n| n.entity_id.starts_with("ks1|person|")),
        "clear literals must use stable global ids: {:?}",
        nodes
    );
    upsert_graph_entities(store.connection(), store.runtime(), &nodes).expect("upsert entities");
    upsert_graph_edges(store.connection(), store.runtime(), &edges).expect("upsert edges");

    let entities_batch = store
        .runtime()
        .block_on(load_graph_entities_batch(store.connection(), "scope-a"))
        .expect("load graph entities");
    let refs = load_entity_refs(&entities_batch).expect("project entity refs");
    assert_eq!(refs.len(), 2, "the Lance round-trip must preserve both nodes");
    assert!(refs.iter().any(|r| r.normalized_literal == "ada lovelace"));
    assert!(refs.iter().any(|r| r.normalized_literal == "charles babbage"));

    let edges_batch = store
        .runtime()
        .block_on(load_graph_edges_batch(store.connection(), "scope-a"))
        .expect("load graph edges");
    let (src, dst) = load_edge_endpoints(&edges_batch).expect("project edge endpoints");
    assert_eq!(src.len(), 1, "one co-occurrence edge expected");
    assert!(refs.iter().any(|r| r.entity_id == src[0]));
    assert!(refs.iter().any(|r| r.entity_id == dst[0]));

    let seed = refs
        .iter()
        .find(|r| r.normalized_literal == "ada lovelace")
        .expect("seed entity")
        .entity_id
        .clone();
    let expanded = expand_entity_graph(&src, &dst, std::slice::from_ref(&seed), 2, 16);
    assert_eq!(expanded.len(), 2, "seeded expansion must reach both entities");
    assert_eq!(expanded[0].entity_id, seed);
    assert_eq!(expanded[0].hop, 0);
    assert_eq!(expanded[1].hop, 1, "the co-occurring entity is one hop away");
    assert_eq!(expanded[1].chain, vec![seed, expanded[1].entity_id.clone()]);
}

/// GraphRAG fusion keeps graph-only chunks (the whole point of entity expansion) and boosts a
/// vector hit that the graph also reached, while staying bounded by `max_results`.
#[test]
fn graphrag_fusion_returns_graph_only_hits_and_bounded_results() {
    let vector = super::super::DocumentHit {
        path: "matter/notes.md".into(),
        chunk_idx: 0,
        text: "Ada Lovelace and Charles Babbage corresponded.".into(),
        mime_type: "text/markdown".into(),
        facets: Default::default(),
        byte_start: 0,
        byte_end: 48,
        distance: 0.25,
    };
    let graph_only = super::super::DocumentHit {
        path: "matter/contract.md".into(),
        chunk_idx: 3,
        text: "The parties agreed on a cure period.".into(),
        mime_type: "text/markdown".into(),
        facets: Default::default(),
        byte_start: 100,
        byte_end: 140,
        distance: 0.0,
    };
    let graph_chunks = vec![
        (vector.clone(), 1u8, vec!["ks1|person|ada lovelace".to_string()]),
        (
            graph_only.clone(),
            2u8,
            vec![
                "ks1|person|ada lovelace".to_string(),
                "ks1|person|charles babbage".to_string(),
            ],
        ),
    ];
    let (hits, truncated) = fuse_results(vec![vector.clone()], graph_chunks, 10);
    assert!(!truncated);
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].path, "matter/notes.md", "graph agreement boosts the vector hit");
    assert_eq!(hits[0].graph_path, vec!["ks1|person|ada lovelace"]);
    assert_eq!(hits[1].path, "matter/contract.md", "graph-only chunk is still returned");
    assert_eq!(hits[1].graph_path.len(), 2);
    assert!(
        hits[1].distance > hits[0].distance,
        "hop 2 scores below a fused vector hit"
    );

    let (few, truncated) = fuse_results(vec![], vec![(graph_only.clone(), 1, vec![]), (vector, 2, vec![])], 1);
    assert!(truncated, "more candidates than max_results must be reported");
    assert_eq!(few.len(), 1);
    assert_eq!(few[0].path, "matter/contract.md", "hop 1 outranks hop 2");
}
