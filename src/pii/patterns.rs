//! Canonical regex tables: EU national IDs and code-security credentials.
//! `RedactionConfig` helpers in `config::documents::pii_patterns` must mirror
//! these entries (enforced by `facade_sync` smoke tests).

use super::SensitivityTier;

pub struct EuNationalIdPattern {
    pub label: &'static str,
    pub country: &'static str,
    pub regex: &'static str,
}

pub const EU_NATIONAL_ID_PATTERNS: &[EuNationalIdPattern] = &[
    EuNationalIdPattern {
        label: "national_id_fr",
        country: "FR",
        regex: r"\b[12]\d{2}(?:0[1-9]|1[0-2])(?:\d{2}|2[AB])\d{8}\b",
    },
    EuNationalIdPattern {
        label: "national_id_nl",
        country: "NL",
        regex: r"\b\d{8,9}\b",
    },
    EuNationalIdPattern {
        label: "national_id_be",
        country: "BE",
        regex: r"\b\d{11}\b",
    },
    EuNationalIdPattern {
        label: "national_id_at",
        country: "AT",
        regex: r"\b\d{10}\b",
    },
    EuNationalIdPattern {
        label: "national_id_ie",
        country: "IE",
        regex: r"\b\d{7}[A-Z]{1,2}\b",
    },
    EuNationalIdPattern {
        label: "national_id_pt",
        country: "PT",
        regex: r"\b\d{9}\b",
    },
];

#[derive(Debug, Clone)]
pub struct CodeSecurityPattern {
    pub label: &'static str,
    pub regex: &'static str,
    pub sensitivity: f32,
}

pub const CODE_SECURITY_PATTERNS: &[CodeSecurityPattern] = &[
    CodeSecurityPattern {
        label: "aws_access_key",
        regex: r"AKIA[0-9A-Z]{16}",
        sensitivity: SensitivityTier::HARD_BLOCK,
    },
    CodeSecurityPattern {
        label: "aws_secret_key",
        regex: r#"(?i)aws_secret_access_key[=\s:]+[^\s"']{20,}"#,
        sensitivity: SensitivityTier::HARD_BLOCK,
    },
    CodeSecurityPattern {
        label: "gcp_credentials",
        regex: r"(?i)ya29\.[0-9A-Za-z_-]+",
        sensitivity: SensitivityTier::HARD_BLOCK,
    },
    CodeSecurityPattern {
        label: "azure_credentials",
        regex: r#"(?i)(client_id|appId|client_secret|app_secret)[=\s:]+["']?[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}["']?"#,
        sensitivity: SensitivityTier::HARD_BLOCK,
    },
    CodeSecurityPattern {
        label: "api_key",
        regex: r#"(?i)(api[_-]?key|apikey|api_secret)[=\s:]+["']?[A-Za-z0-9_\-]{16,}["']?"#,
        sensitivity: SensitivityTier::HARD_BLOCK,
    },
    CodeSecurityPattern {
        label: "jwt_token",
        regex: r"eyJ[A-Za-z0-9_-]+\.eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+",
        sensitivity: SensitivityTier::HARD_BLOCK,
    },
    CodeSecurityPattern {
        label: "bearer_token",
        regex: r"(?i)Bearer\s+(eyJ|tok_[a-zA-Z0-9]+|sk_[a-zA-Z0-9]+|[A-Za-z0-9_-]{20,})",
        sensitivity: SensitivityTier::HARD_BLOCK,
    },
    CodeSecurityPattern {
        label: "oauth_token",
        regex: r#"(?i)(access_token|refresh_token)[=\s:]+["']?[A-Za-z0-9_.\-]{10,}["']?"#,
        sensitivity: SensitivityTier::HARD_BLOCK,
    },
    CodeSecurityPattern {
        label: "ssh_private_key",
        regex: r"-----BEGIN\s+(OPENSSH|RSA|EC|DSA|GPG) PRIVATE KEY-----",
        sensitivity: SensitivityTier::HARD_BLOCK,
    },
    CodeSecurityPattern {
        label: "gpg_private_key",
        regex: r"-----BEGIN PGP PRIVATE KEY BLOCK-----",
        sensitivity: SensitivityTier::HARD_BLOCK,
    },
    CodeSecurityPattern {
        label: "tls_certificate",
        regex: r"-----BEGIN CERTIFICATE-----",
        sensitivity: SensitivityTier::HARD_BLOCK,
    },
    CodeSecurityPattern {
        label: "db_connection_string",
        regex: r"(?i)(postgresql|mysql|mongodb|jdbc|mssql|redis)://[^\s@]+:[^\s@]+@[^\s/]+",
        sensitivity: SensitivityTier::HARD_BLOCK,
    },
    CodeSecurityPattern {
        label: "env_secret",
        regex: r#"(?i)(?:export\s+)?(?:API_KEY|SECRET|PASSWORD|TOKEN|PRIVATE|KEY|CREDENTIALS|ACCESS_KEY|Auth)=(?:['"]?)[A-Za-z0-9_/\-+=.]{8,}(?:['"]?)"#,
        sensitivity: SensitivityTier::HARD_BLOCK,
    },
    CodeSecurityPattern {
        label: "ipv4_private",
        regex: r"\b(?:(?:10\.|172\.(?:1[6-9]|2[0-9]|3[01])\.|192\.168\.)[0-9]{1,3}\.[0-9]{1,3}|127\.[0-9]+\.[0-9]+\.[0-9]+|169\.254\.[0-9]+\.[0-9]+)\b",
        sensitivity: SensitivityTier::MEDIUM,
    },
    CodeSecurityPattern {
        label: "ipv6_private",
        regex: r"(?i)\b(?:[fF][cCdD][0-9a-fA-F]{2}:[0-9a-fA-F:]{1,39}|fe80:[0-9a-fA-F:]{1,39})\b|(?:^|[^0-9a-fA-F:])::1\b",
        sensitivity: SensitivityTier::MEDIUM,
    },
    CodeSecurityPattern {
        label: "internal_hostname",
        regex: r"(?i)\b(?:[a-z0-9](?:[a-z0-9-]*[a-z0-9])?\.)*(?:internal|corp|local|intranet|private|dmz|lan)(?:\.[a-z]{2,})?\b",
        sensitivity: SensitivityTier::MEDIUM,
    },
    CodeSecurityPattern {
        label: "internal_url",
        regex: r#"(?i)https?://(?:(?:10\.|172\.(?:1[6-9]|2[0-9]|3[01])\.|192\.168\.)[0-9]{1,3}(?:\.[0-9]{1,3})?(?::\d+)?|(?:[a-z0-9-]+\.)*(?:internal|corp|local|intranet|private|dmz)\.[a-z]{2,}(?::\d+)?)/[^\s"']*"#,
        sensitivity: SensitivityTier::MEDIUM,
    },
    CodeSecurityPattern {
        label: "mac_address",
        regex: r"\b(?:[0-9a-fA-F]{2}:){5}[0-9a-fA-F]{2}\b",
        sensitivity: SensitivityTier::LOW,
    },
    CodeSecurityPattern {
        label: "cookie_id",
        regex: r#"(?i)(?:session[_-]?id|session[_-]?token|cookie)[=\s]+["']?[A-Za-z0-9_.\-]{10,}["']?"#,
        sensitivity: SensitivityTier::LOW,
    },
];

pub const IBAN_REGEX: &str = r"\b[A-Z]{2}\d{2}[A-Z0-9]{11,30}\b";
