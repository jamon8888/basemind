use super::*;

#[test]
fn secret_string_redacts_in_debug() {
    let s = SecretString::new("hunter2".to_string());
    assert_eq!(format!("{s:?}"), "\"<redacted>\"");
    assert_eq!(format!("{s}"), "<redacted>");
    assert_eq!(s.expose(), "hunter2");
}

#[test]
fn api_key_unset_resolves_to_none() {
    assert!(ApiKey::Unset.resolve().is_none());
}

#[test]
fn api_key_literal_resolves_to_value() {
    let k = ApiKey::Literal("sk-test".to_string());
    let resolved = k.resolve().expect("literal resolves");
    assert_eq!(resolved.expose(), "sk-test");
}

#[test]
fn api_key_literal_empty_resolves_to_none() {
    assert!(ApiKey::Literal(String::new()).resolve().is_none());
}

#[test]
fn api_key_env_reads_environment() {
    // SAFETY: scoped to this single test; the env var name is uniquely
    unsafe {
        std::env::set_var("BASEMIND_TEST_API_KEY_PRESENT", "value-123");
    }
    let k = ApiKey::Env {
        env: "BASEMIND_TEST_API_KEY_PRESENT".to_string(),
    };
    let resolved = k.resolve().expect("env resolves");
    assert_eq!(resolved.expose(), "value-123");
    unsafe {
        std::env::remove_var("BASEMIND_TEST_API_KEY_PRESENT");
    }
}

#[test]
fn api_key_env_missing_resolves_to_none() {
    // SAFETY: the variable is removed before the test so the env lookup
    unsafe {
        std::env::remove_var("BASEMIND_TEST_API_KEY_MISSING");
    }
    let k = ApiKey::Env {
        env: "BASEMIND_TEST_API_KEY_MISSING".to_string(),
    };
    assert!(k.resolve().is_none());
}

#[test]
fn api_key_deserialises_literal_string() {
    let k: ApiKey = serde_json::from_str("\"sk-test\"").expect("parse");
    match k {
        ApiKey::Literal(s) => assert_eq!(s, "sk-test"),
        other => panic!("expected Literal, got {other:?}"),
    }
}

#[test]
fn api_key_deserialises_env_table() {
    let k: ApiKey = serde_json::from_str(r#"{"env":"OPENAI_API_KEY"}"#).expect("parse");
    match k {
        ApiKey::Env { env } => assert_eq!(env, "OPENAI_API_KEY"),
        other => panic!("expected Env, got {other:?}"),
    }
}

#[test]
fn api_key_literal_never_serializes_cleartext() {
    let key = ApiKey::Literal("sk-supersecret".to_string());
    let json = serde_json::to_string(&key).expect("serialize");
    assert!(!json.contains("sk-supersecret"), "raw secret leaked: {json}");
    assert!(json.contains("<redacted>"), "redaction marker missing: {json}");
}

#[test]
fn api_key_env_serializes_env_name_only() {
    let key = ApiKey::Env {
        env: "OPENAI_API_KEY".to_string(),
    };
    let json = serde_json::to_string(&key).expect("serialize");
    assert!(json.contains("OPENAI_API_KEY"), "env name missing: {json}");
    assert!(json.contains("\"env\""), "env field missing: {json}");
}

#[test]
fn api_key_unset_serializes_to_null() {
    let key = ApiKey::Unset;
    let json = serde_json::to_string(&key).expect("serialize");
    assert_eq!(json, "null");
}

#[test]
fn reranker_defaults_follow_eu_spec() {
    let r = RerankerConfig::default();
    assert!(!r.enabled);
    assert_eq!(r.preset, "bge-reranker-v2-m3");
    assert_eq!(r.top_k, 20);
    assert!(r.custom_model.is_none());
}

#[test]
fn reranker_resolve_explicit_preset_wins_over_custom() {
    let r = RerankerConfig {
        custom_model: Some(CustomRerankerModel {
            model_id: "onnx-community/gte-multilingual-reranker-base".to_string(),
            model_file: Some("onnx/model_int8.onnx".to_string()),
        }),
        ..RerankerConfig::default()
    };
    match r.resolve_model(Some("bge-reranker-base")) {
        ResolvedRerankerModel::Preset { name } => assert_eq!(name, "bge-reranker-base"),
        other => panic!("expected Preset, got {other:?}"),
    }
}

#[test]
fn reranker_resolve_custom_without_override() {
    let r = RerankerConfig {
        custom_model: Some(CustomRerankerModel {
            model_id: "onnx-community/gte-multilingual-reranker-base".to_string(),
            model_file: Some("onnx/model_int8.onnx".to_string()),
        }),
        ..RerankerConfig::default()
    };
    match r.resolve_model(None) {
        ResolvedRerankerModel::Custom {
            model_id, model_file, ..
        } => {
            assert_eq!(model_id, "onnx-community/gte-multilingual-reranker-base");
            assert_eq!(model_file, "onnx/model_int8.onnx");
        }
        other => panic!("expected Custom, got {other:?}"),
    }
}

#[test]
fn reranker_resolve_custom_defaults_model_file() {
    let r = RerankerConfig {
        custom_model: Some(CustomRerankerModel {
            model_id: "org/model".to_string(),
            model_file: None,
        }),
        ..RerankerConfig::default()
    };
    match r.resolve_model(None) {
        ResolvedRerankerModel::Custom { model_file, .. } => {
            assert_eq!(model_file, "onnx/model.onnx");
        }
        other => panic!("expected Custom, got {other:?}"),
    }
}

#[test]
fn reranker_resolve_falls_back_to_preset() {
    let r = RerankerConfig::default();
    match r.resolve_model(None) {
        ResolvedRerankerModel::Preset { name } => assert_eq!(name, "bge-reranker-v2-m3"),
        other => panic!("expected Preset, got {other:?}"),
    }
}

#[test]
fn reranker_custom_model_deserialises_from_toml_section() {
    let cfg: RerankerConfig = toml::from_str(
            "preset = \"bge-reranker-v2-m3\"\n[custom_model]\nmodel_id = \"onnx-community/gte-multilingual-reranker-base\"\nmodel_file = \"onnx/model_int8.onnx\"\n",
        )
        .expect("parse");
    let custom = cfg.custom_model.expect("custom_model");
    assert_eq!(custom.model_id, "onnx-community/gte-multilingual-reranker-base");
    assert_eq!(custom.model_file.as_deref(), Some("onnx/model_int8.onnx"));
}

#[test]
fn ocr_defaults_cover_eu_id_languages() {
    let o = OcrConfig::default();
    for lang in ["eng", "deu", "fra", "spa", "ita", "nld", "pol"] {
        assert!(o.languages.contains(&lang.to_string()), "missing {lang}");
    }
}

#[test]
fn embedding_preset_default_is_multilingual() {
    // Changing the preset changes the LanceDB dim triple and wipes every
    // existing index — pick the default deliberately before first index.
    assert_eq!(DocumentsConfig::default().embedding_preset, "multilingual");
}
