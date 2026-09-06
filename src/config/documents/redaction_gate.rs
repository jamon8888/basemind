//! Provider gate for the GDPR redaction pipeline.
//!
//! Decides whether pseudonymization applies for the configured LLM provider:
//! local-only inference never leaves the machine, so redaction latency buys no
//! privacy there, while hosted providers always pseudonymize. Consulted at the
//! single xberg handoff; unknown or empty providers fail closed (applies).

use super::RedactionConfig;

/// Extract the provider half of a liter-llm routing string (`"provider/model"`).
/// No slash returns the whole string; empty returns empty.
pub fn llm_provider(model: &str) -> &str {
    model.split('/').next().unwrap_or("")
}

impl RedactionConfig {
    /// Master gate for pseudonymization: false when the pipeline is disabled,
    /// false for local providers unless bypass is off, true otherwise.
    pub fn applies_to_provider(&self, llm_model: &str) -> bool {
        if !self.enabled {
            return false;
        }
        if self.bypass_local_providers
            && self
                .local_providers
                .iter()
                .any(|p| p.eq_ignore_ascii_case(llm_provider(llm_model)))
        {
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enabled() -> RedactionConfig {
        RedactionConfig {
            enabled: true,
            ..Default::default()
        }
    }

    #[test]
    fn provider_parsing() {
        assert_eq!(llm_provider("ollama/llama3.1"), "ollama");
        assert_eq!(llm_provider("openai/gpt-4o"), "openai");
        assert_eq!(llm_provider("gpt-4o"), "gpt-4o");
        assert_eq!(llm_provider(""), "");
    }

    #[test]
    fn disabled_never_applies() {
        let cfg = RedactionConfig::default();
        assert!(!cfg.applies_to_provider("ollama/llama3.1"));
        assert!(!cfg.applies_to_provider("openai/gpt-4o"));
        assert!(!cfg.applies_to_provider(""));
    }

    #[test]
    fn local_provider_bypassed_when_enabled() {
        let cfg = RedactionConfig {
            enabled: true,
            bypass_local_providers: true,
            ..Default::default()
        };
        assert!(!cfg.applies_to_provider("ollama/llama3.1"));
        assert!(cfg.applies_to_provider("openai/gpt-4o"));
    }

    #[test]
    fn routing_dormant_by_default() {
        // Default preserves current behavior: enabled redacts for every provider.
        let cfg = enabled();
        assert!(cfg.applies_to_provider("ollama/llama3.1"));
        assert!(cfg.applies_to_provider("openai/gpt-4o"));
    }

    #[test]
    fn hosted_and_unknown_fail_closed() {
        let cfg = enabled();
        assert!(cfg.applies_to_provider("openai/gpt-4o"));
        assert!(cfg.applies_to_provider("anthropic/claude-sonnet-4-20250514"));
        assert!(cfg.applies_to_provider(""));
    }

    #[test]
    fn custom_local_list() {
        let cfg = RedactionConfig {
            enabled: true,
            bypass_local_providers: true,
            local_providers: vec!["vllm".to_string()],
            ..Default::default()
        };
        assert!(!cfg.applies_to_provider("vllm/qwen"));
        assert!(cfg.applies_to_provider("ollama/llama3.1"));
    }

    #[test]
    fn provider_match_is_case_insensitive() {
        let cfg = RedactionConfig {
            enabled: true,
            bypass_local_providers: true,
            ..Default::default()
        };
        assert!(!cfg.applies_to_provider("Ollama/llama3.1"));
    }
}
