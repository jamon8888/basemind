//! PII pattern helpers for the redaction config.
//!
//! Extracted from [`super::RedactionConfig`] to keep the parent module
//! under the 1000-line cap.

use super::RedactionCustomPattern;

/// Returns all EU national ID patterns as `RedactionCustomPattern` entries,
/// ready to append to `custom_patterns`. These cover the 6 EU countries missing
/// from xberg's built-in set: FR NIR, NL BSN, BE NISS, AT SVNR, IE PPS, PT NIF.
///
/// Checksum validation runs post-extraction via [`crate::pii::validate_eu_national_id`];
/// the regex patterns are conservative to avoid false positives and the checksum
/// filter eliminates invalid matches.
pub fn eu_national_id_patterns() -> Vec<RedactionCustomPattern> {
    vec![
        RedactionCustomPattern {
            label: "national_id_fr".into(),
            pattern: r"\b[12]\d{2}(?:0[1-9]|1[0-2])\d{10}\b".into(),
            case_sensitive: false,
        },
        RedactionCustomPattern {
            label: "national_id_nl".into(),
            pattern: r"\b\d{8,9}\b".into(),
            case_sensitive: false,
        },
        RedactionCustomPattern {
            label: "national_id_be".into(),
            pattern: r"\b\d{11}\b".into(),
            case_sensitive: false,
        },
        RedactionCustomPattern {
            label: "national_id_at".into(),
            pattern: r"\b\d{10}\b".into(),
            case_sensitive: false,
        },
        RedactionCustomPattern {
            label: "national_id_ie".into(),
            pattern: r"\b\d{7}[A-Z]{1,2}\b".into(),
            case_sensitive: false,
        },
        RedactionCustomPattern {
            label: "national_id_pt".into(),
            pattern: r"\b\d{9}\b".into(),
            case_sensitive: false,
        },
    ]
}

/// Returns all code security patterns as `RedactionCustomPattern` entries,
/// ready to append to `custom_patterns`. These cover credentials and technical
/// identifiers that xberg's built-in NER does not handle.
///
/// All of these are hard-block sensitivity (1.0) — secret leakage is a critical
/// finding, not merely a privacy concern.
pub fn code_security_patterns() -> Vec<RedactionCustomPattern> {
    vec![
        RedactionCustomPattern {
            label: "aws_access_key".into(),
            pattern: r"AKIA[0-9A-Z]{16}".into(),
            case_sensitive: false,
        },
        RedactionCustomPattern {
            label: "aws_secret_key".into(),
            pattern: r#"(?i)aws_secret_access_key[=\s:]+[^\s"']{20,}"#.into(),
            case_sensitive: false,
        },
        RedactionCustomPattern {
            label: "gcp_credentials".into(),
            pattern: r"(?i)ya29\.[0-9A-Za-z_-]+".into(),
            case_sensitive: false,
        },
        RedactionCustomPattern {
            label: "azure_credentials".into(),
            pattern: r#"(?i)(client_id|appId|client_secret|app_secret)[=\s:]+["']?[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}["']?"#
                .into(),
            case_sensitive: false,
        },
        RedactionCustomPattern {
            label: "api_key".into(),
            pattern: r#"(?i)(api[_-]?key|apikey|api_secret)[=\s:]+["']?[A-Za-z0-9_\-]{16,}["']?"#
                .into(),
            case_sensitive: false,
        },
        RedactionCustomPattern {
            label: "jwt_token".into(),
            pattern: r"eyJ[A-Za-z0-9_-]+\.eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+".into(),
            case_sensitive: false,
        },
        RedactionCustomPattern {
            label: "bearer_token".into(),
            pattern: r"(?i)Bearer\s+(eyJ|tok_[a-zA-Z0-9]+|sk_[a-zA-Z0-9]+|[A-Za-z0-9_-]{20,})"
                .into(),
            case_sensitive: false,
        },
        RedactionCustomPattern {
            label: "oauth_token".into(),
            pattern: r#"(?i)(access_token|refresh_token)[=\s:]+["']?[A-Za-z0-9_.\-]{10,}["']?"#
                .into(),
            case_sensitive: false,
        },
        RedactionCustomPattern {
            label: "ssh_private_key".into(),
            pattern: r"-----BEGIN\s+(OPENSSH|RSA|EC|DSA|GPG) PRIVATE KEY-----".into(),
            case_sensitive: false,
        },
        RedactionCustomPattern {
            label: "gpg_private_key".into(),
            pattern: r"-----BEGIN PGP PRIVATE KEY BLOCK-----".into(),
            case_sensitive: false,
        },
        RedactionCustomPattern {
            label: "tls_certificate".into(),
            pattern: r"-----BEGIN CERTIFICATE-----".into(),
            case_sensitive: false,
        },
        RedactionCustomPattern {
            label: "db_connection_string".into(),
            pattern: r"(?i)(postgresql|mysql|mongodb|jdbc|mssql|redis)://[^\s@]+:[^\s@]+@[^\s/]+"
                .into(),
            case_sensitive: false,
        },
        RedactionCustomPattern {
            label: "env_secret".into(),
            pattern: r#"(?i)(?:export\s+)?(?:API_KEY|SECRET|PASSWORD|TOKEN|PRIVATE|KEY|CREDENTIALS|ACCESS_KEY|Auth)=(?:['"]?)[A-Za-z0-9_/\-+=.]{8,}(?:['"]?)"#
                .into(),
            case_sensitive: false,
        },
    ]
}
