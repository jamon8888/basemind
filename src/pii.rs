//! PII detection patterns for GDPR-compliant EU document processing.

use serde::{Deserialize, Serialize};

mod patterns;
mod pipeline;
mod translation;
mod validators;

pub use translation::{ChunkSpan, FindingInput, TranslationStats, translate_findings};

pub use patterns::{
    CODE_SECURITY_PATTERNS, CodeSecurityPattern, EU_NATIONAL_ID_PATTERNS, EuNationalIdPattern, IBAN_REGEX,
};
pub use pipeline::{
    DetectedSpan, ERASED_VALUE_HASH, confidence_threshold, dedupe_spans, gliner_label_threshold, is_private_ipv4,
    passes_threshold, risk_for_label, validate_db_connection_string, validate_e164, validate_jwt,
};
pub use validators::{
    validate_at_svnr, validate_be_niss, validate_eu_national_id, validate_fr_nir, validate_iban, validate_ie_pps,
    validate_nl_bsn, validate_pt_nif,
};

/// Sensitivity tier for reranking suppression (post-RRF).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SensitivityTier(pub f32);

impl SensitivityTier {
    pub const HARD_BLOCK: f32 = 1.0;
    pub const HIGH: f32 = 0.9;
    pub const MEDIUM: f32 = 0.7;
    pub const LOW: f32 = 0.4;
    pub const MINIMAL: f32 = 0.2;
}

impl std::ops::Deref for SensitivityTier {
    type Target = f32;
    fn deref(&self) -> &f32 {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PiiCategory {
    PersonFullName,
    PersonFirstName,
    PersonLastName,
    DateOfBirth,
    Email,
    PhoneNumber,
    Address,
    City,
    PostalCode,
    NationalIdFr,
    NationalIdNl,
    NationalIdBe,
    NationalIdAt,
    NationalIdIe,
    NationalIdPt,
    NationalIdGeneric,
    Iban,
    CreditCard,
    BankAccount,
    PassportNumber,
    DriversLicense,
    TaxId,
    AwsAccessKey,
    AwsSecretKey,
    GcpCredentials,
    AzureCredentials,
    ApiKey,
    JwtToken,
    BearerToken,
    OAuthToken,
    SshPrivateKey,
    GpgPrivateKey,
    TlsCertificate,
    DbConnectionString,
    EnvSecret,
    IpAddress,
    InternalHostname,
    InternalUrl,
    MacAddress,
    CookieId,
    Organization,
    Location,
}

impl PiiCategory {
    pub fn eu_national_id_labels() -> Vec<&'static str> {
        vec![
            "national_id_fr",
            "national_id_nl",
            "national_id_be",
            "national_id_at",
            "national_id_ie",
            "national_id_pt",
            "national_id_generic",
        ]
    }
    pub fn code_security_labels() -> Vec<&'static str> {
        vec![
            "aws_access_key",
            "aws_secret_key",
            "gcp_credentials",
            "azure_credentials",
            "api_key",
            "jwt_token",
            "bearer_token",
            "oauth_token",
            "ssh_private_key",
            "gpg_private_key",
            "tls_certificate",
            "db_connection_string",
            "env_secret",
            "internal_url",
            "internal_hostname",
            "ipv4_private",
            "ipv6_private",
            "mac_address",
            "cookie_id",
        ]
    }
}

impl From<&PiiCategory> for f32 {
    fn from(cat: &PiiCategory) -> f32 {
        match cat {
            PiiCategory::DateOfBirth => SensitivityTier::HIGH,
            PiiCategory::NationalIdFr
            | PiiCategory::NationalIdNl
            | PiiCategory::NationalIdBe
            | PiiCategory::NationalIdAt
            | PiiCategory::NationalIdIe
            | PiiCategory::NationalIdPt
            | PiiCategory::NationalIdGeneric
            | PiiCategory::PassportNumber
            | PiiCategory::DriversLicense
            | PiiCategory::TaxId => SensitivityTier::HIGH,
            PiiCategory::Iban | PiiCategory::CreditCard | PiiCategory::BankAccount => SensitivityTier::HIGH,
            PiiCategory::AwsAccessKey
            | PiiCategory::AwsSecretKey
            | PiiCategory::GcpCredentials
            | PiiCategory::AzureCredentials
            | PiiCategory::ApiKey
            | PiiCategory::JwtToken
            | PiiCategory::BearerToken
            | PiiCategory::OAuthToken
            | PiiCategory::SshPrivateKey
            | PiiCategory::GpgPrivateKey
            | PiiCategory::TlsCertificate
            | PiiCategory::DbConnectionString
            | PiiCategory::EnvSecret => SensitivityTier::HARD_BLOCK,
            PiiCategory::InternalUrl | PiiCategory::InternalHostname => SensitivityTier::MEDIUM,
            PiiCategory::Email | PiiCategory::PhoneNumber => SensitivityTier::LOW,
            PiiCategory::IpAddress => SensitivityTier::LOW,
            PiiCategory::MacAddress | PiiCategory::CookieId => SensitivityTier::LOW,
            PiiCategory::PersonFullName | PiiCategory::PersonFirstName | PiiCategory::PersonLastName => {
                SensitivityTier::MINIMAL
            }
            PiiCategory::Organization => SensitivityTier::MINIMAL,
            PiiCategory::Location => SensitivityTier::MINIMAL,
            PiiCategory::Address | PiiCategory::City | PiiCategory::PostalCode => SensitivityTier::LOW,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityLocation {
    pub file_id: String,
    pub chunk_index: i32,
    /// Character offsets when the source text is available; `-1` when only byte
    /// offsets were reported (the redaction engine drops original bytes, so the
    /// translation layer cannot map bytes back to chars).
    pub char_start: i32,
    pub char_end: i32,
    /// Byte offsets in the original content, when reported. `Some` for entities
    /// translated from redaction findings, whose offsets are byte-exact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub byte_start: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub byte_end: Option<i32>,
    pub page_number: Option<i32>,
    pub context: String,
}

fn deserialize_detected_at<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;
    let val = serde_json::Value::deserialize(deserializer)?;
    if let Some(i) = val.as_i64() {
        return Ok(i);
    }
    if let Some(s) = val.as_str() {
        if let Ok(i) = s.parse::<i64>() {
            return Ok(i);
        }
        // fallback: attempt to parse numeric string with whitespace
        let trimmed = s.trim();
        if let Ok(i) = trimmed.parse::<i64>() {
            return Ok(i);
        }
        return Err(D::Error::custom(format!("invalid detected_at string: {s}")));
    }
    Err(D::Error::custom("detected_at must be integer or numeric string"))
}

/// PII entity for GDPR Article 30 accountability.
/// Constructed by the extraction pipeline; `lineage` links parent/child entities
/// (e.g. a chunk → an IBAN → the document it came from) to support audit trails.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PiiEntity {
    pub entity_id: String,
    pub category: String,
    pub subcategory: Option<String>,
    pub value_hash: String,
    pub confidence: f32,
    pub detector_version: String,
    pub locations: Vec<EntityLocation>,
    /// Unix microseconds when the entity was first detected. Integer timestamps
    /// follow the repo convention (no date crate in the tree); the originating
    /// spec's ISO 8601 rendering is a display concern, not a storage one.
    #[serde(deserialize_with = "deserialize_detected_at")]
    pub detected_at: i64,
    /// Identifier of the pipeline stage or worker that processed this entity.
    pub processed_by: String,
    /// Legal basis for processing (e.g. "consent", "contract", "legitimate_interest").
    pub legal_basis: Option<String>,
    /// Optional retention cutoff; entities past this date should be deleted.
    pub retention_until: Option<String>,
    pub risk_level: RiskLevel,
    /// Parent entity IDs forming an audit chain (parent → current entity).
    /// Populated by the extraction pipeline when entities are derived from other entities.
    #[serde(default)]
    pub lineage: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    Critical,
    High,
    Medium,
    Low,
}

impl From<f32> for RiskLevel {
    fn from(s: f32) -> Self {
        if s >= SensitivityTier::HARD_BLOCK {
            RiskLevel::Critical
        } else if s >= SensitivityTier::HIGH {
            RiskLevel::High
        } else if s >= SensitivityTier::MEDIUM {
            RiskLevel::Medium
        } else {
            RiskLevel::Low
        }
    }
}

impl PiiEntity {
    pub fn sensitivity(&self) -> f32 {
        match self.risk_level {
            RiskLevel::Critical => SensitivityTier::HARD_BLOCK,
            RiskLevel::High => SensitivityTier::HIGH,
            RiskLevel::Medium => SensitivityTier::MEDIUM,
            RiskLevel::Low => SensitivityTier::LOW,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RrfWeights {
    pub exact: f32,
    pub keyword: f32,
    pub vector: f32,
}
impl Default for RrfWeights {
    fn default() -> Self {
        Self {
            exact: 3.0,
            keyword: 2.0,
            vector: 1.0,
        }
    }
}
impl RrfWeights {
    pub const PII_WORKLOAD: Self = Self {
        exact: 3.0,
        keyword: 2.0,
        vector: 1.0,
    };
    pub const BALANCED: Self = Self {
        exact: 1.0,
        keyword: 1.0,
        vector: 1.0,
    };
}

pub fn suppress_by_sensitivity(results: &mut [(String, f32)], entity_sensitivities: &[(String, f32)]) {
    let sensitivity_map: std::collections::HashMap<&str, f32> =
        entity_sensitivities.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    for r in results.iter_mut() {
        let max_sens = sensitivity_map.get(r.0.as_str()).copied().unwrap_or(0.0);
        r.1 *= 1.0 - 0.9 * max_sens;
    }
    results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fr_nir_valid() {
        assert!(validate_fr_nir("185071510000058"));
    }
    #[test]
    fn test_fr_nir_invalid() {
        assert!(!validate_fr_nir("285071512346"));
        assert!(!validate_fr_nir("18507151234668"));
    }
    #[test]
    fn test_fr_nir_corsica() {
        // Corsica department codes 2A/2B substitute 19/18 (stdnum vector).
        assert!(validate_fr_nir("185072A10000047"));
        assert!(!validate_fr_nir("185072A10000058"));
    }
    #[test]
    fn test_nl_bsn_valid() {
        assert!(validate_nl_bsn("100000009"));
        assert!(validate_nl_bsn("10000008"));
    }
    #[test]
    fn test_nl_bsn_invalid() {
        assert!(!validate_nl_bsn("123456789"));
        assert!(!validate_nl_bsn("100000002"));
        assert!(!validate_nl_bsn("1234567"));
    }
    #[test]
    fn test_be_niss_valid() {
        // Vectors from python-stdnum `be.nn` docstrings.
        assert!(validate_be_niss("85073003328"));
        assert!(validate_be_niss("17073003384"));
    }
    #[test]
    fn test_be_niss_invalid() {
        assert!(!validate_be_niss("12345678901"));
        assert!(!validate_be_niss("00012512321"));
        assert!(!validate_be_niss("85073003329"));
    }
    #[test]
    fn test_at_svnr_valid() {
        assert!(validate_at_svnr("7829280755"));
    }
    #[test]
    fn test_ie_pps_valid() {
        // Vectors from python-stdnum `ie.pps` docstrings.
        assert!(validate_ie_pps("6433435F")); // pre-2013
        assert!(validate_ie_pps("6433435FT")); // pre-2013 with special final 'T'
        assert!(validate_ie_pps("6433435FW")); // pre-2013 married-women format
        assert!(validate_ie_pps("6433435OA")); // 2013 personal format
        assert!(validate_ie_pps("6433435IH")); // 2013 non-personal format
    }
    #[test]
    fn test_ie_pps_invalid() {
        assert!(!validate_ie_pps("6433435E")); // wrong check digit
        assert!(!validate_ie_pps("6433435VH")); // wrong check digit, 2013 format
        assert!(!validate_ie_pps("1234567U"));
        assert!(!validate_ie_pps("1234567"));
    }
    #[test]
    fn test_pt_nif_valid() {
        assert!(validate_pt_nif("200000039"));
    }
    #[test]
    fn test_iban_valid() {
        assert!(validate_iban("DE89370400440532013000"));
        assert!(validate_iban("FR1420041010050500013M02606"));
        assert!(validate_iban("NL91ABNA0417164300"));
        assert!(validate_iban("IS140159260076545510730339"));
        assert!(validate_iban("AD1200012030200359100100"));
    }
    #[test]
    fn test_iban_invalid() {
        assert!(!validate_iban("DE89370400440532013001"));
        assert!(!validate_iban("XX89370400440532013000"));
        assert!(!validate_iban("DE89"));
    }
    #[test]
    fn test_sensitivity_tier_from_category() {
        assert_eq!(f32::from(&PiiCategory::AwsAccessKey), SensitivityTier::HARD_BLOCK);
        assert_eq!(f32::from(&PiiCategory::NationalIdFr), SensitivityTier::HIGH);
        assert_eq!(f32::from(&PiiCategory::Email), SensitivityTier::LOW);
        assert_eq!(f32::from(&PiiCategory::PersonFullName), SensitivityTier::MINIMAL);
    }
    #[test]
    fn test_risk_level_from_sensitivity() {
        assert_eq!(RiskLevel::from(SensitivityTier::HARD_BLOCK), RiskLevel::Critical);
        assert_eq!(RiskLevel::from(SensitivityTier::HIGH), RiskLevel::High);
        assert_eq!(RiskLevel::from(SensitivityTier::MEDIUM), RiskLevel::Medium);
        assert_eq!(RiskLevel::from(SensitivityTier::LOW), RiskLevel::Low);
    }
    #[test]
    fn test_suppress_hard_block() {
        let mut results = vec![("chunk_a".to_string(), 0.9_f32), ("chunk_b".to_string(), 0.8_f32)];
        let sensitivities = vec![("chunk_a".to_string(), SensitivityTier::HARD_BLOCK)];
        suppress_by_sensitivity(&mut results, &sensitivities);
        assert!((results[1].1 - 0.09_f32).abs() < 1e-6);
    }
    #[test]
    fn test_suppress_no_sensitivity() {
        let mut results = vec![("chunk_a".to_string(), 0.9_f32)];
        suppress_by_sensitivity(&mut results, &[]);
        assert!((results[0].1 - 0.9_f32).abs() < 1e-6);
    }
    #[test]
    fn test_validate_eu_national_id_dispatch() {
        assert!(validate_eu_national_id("national_id_fr", "185071510000058"));
        assert!(!validate_eu_national_id("national_id_fr", "18507151234668"));
        assert!(validate_eu_national_id("national_id_nl", "100000009"));
        assert!(!validate_eu_national_id("national_id_nl", "123456789"));
        assert!(validate_eu_national_id("unknown", "123"));
    }
    #[test]
    fn test_validate_iban_de() {
        assert!(validate_iban("DE89370400440532013000"));
    }
    #[test]
    fn test_all_code_security_patterns_valid_regex() {
        for pat in CODE_SECURITY_PATTERNS {
            regex::Regex::new(pat.regex).unwrap_or_else(|_| panic!("invalid regex for {}", pat.label));
        }
    }
    #[test]
    fn test_all_eu_national_id_patterns_valid_regex() {
        for pat in EU_NATIONAL_ID_PATTERNS {
            regex::Regex::new(pat.regex).unwrap_or_else(|_| panic!("invalid regex for {}", pat.label));
        }
    }
    #[test]
    fn test_confidence_thresholds_per_spec() {
        assert_eq!(confidence_threshold("national_id_fr"), 0.95);
        assert_eq!(confidence_threshold("iban"), 0.95);
        assert_eq!(confidence_threshold("credit_card"), 0.95);
        assert_eq!(confidence_threshold("api_key"), 0.95);
        assert_eq!(confidence_threshold("jwt_token"), 0.95);
        assert_eq!(confidence_threshold("health_data"), 0.95);
        assert_eq!(confidence_threshold("internal_hostname"), 0.90);
        assert_eq!(confidence_threshold("email"), 0.90);
        assert_eq!(confidence_threshold("phone"), 0.85);
        assert_eq!(confidence_threshold("ip_address"), 0.85);
        assert_eq!(confidence_threshold("person_name"), 0.75);
        assert_eq!(confidence_threshold("organization"), 0.70);
        assert_eq!(confidence_threshold("location"), 0.70);
    }
    #[test]
    fn test_passes_threshold_boundary() {
        assert!(passes_threshold("iban", 0.95));
        assert!(!passes_threshold("iban", 0.949));
        assert!(passes_threshold("person_name", 0.75));
        assert!(!passes_threshold("person_name", 0.74));
    }
    #[test]
    fn test_risk_for_label_tiers() {
        assert_eq!(risk_for_label("api_key"), RiskLevel::Critical);
        assert_eq!(risk_for_label("jwt_token"), RiskLevel::Critical);
        assert_eq!(risk_for_label("national_id_fr"), RiskLevel::High);
        assert_eq!(risk_for_label("iban"), RiskLevel::High);
        assert_eq!(risk_for_label("email"), RiskLevel::Medium);
        assert_eq!(risk_for_label("internal_url"), RiskLevel::Medium);
        assert_eq!(risk_for_label("person_name"), RiskLevel::Low);
        assert_eq!(risk_for_label("something_new"), RiskLevel::Low);
    }
    #[test]
    fn test_gliner_label_thresholds_per_spec() {
        assert_eq!(gliner_label_threshold("full_name"), 0.7);
        assert_eq!(gliner_label_threshold("person"), 0.7);
        assert_eq!(gliner_label_threshold("api_key"), 0.3);
        assert_eq!(gliner_label_threshold("password"), 0.3);
        assert_eq!(gliner_label_threshold("iban"), 0.3);
        assert_eq!(gliner_label_threshold("ip_address"), 0.3);
    }
    #[test]
    fn test_validate_e164() {
        assert!(validate_e164("+33612345678"));
        assert!(validate_e164("+1 415 555 2671"));
        assert!(validate_e164("+49-170-1234567"));
        assert!(!validate_e164("0612345678")); // missing +
        assert!(!validate_e164("+012345678")); // leading zero
        assert!(!validate_e164("+123")); // too short
        assert!(!validate_e164("not a phone"));
    }
    #[test]
    fn test_validate_jwt() {
        assert!(validate_jwt(
            "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0In0.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c"
        ));
        assert!(!validate_jwt("not.a.jwt.at.all.extra"));
        assert!(!validate_jwt("eyJhbGciOiJIUzI1NiJ9.only-two"));
        assert!(!validate_jwt("abc.def.ghi")); // header must start with eyJ
        assert!(!validate_jwt("eyJ.abc.def=ghi.jkl")); // = only as trailing padding
        assert!(!validate_jwt(""));
    }
    #[test]
    fn test_is_private_ipv4() {
        assert!(is_private_ipv4("10.0.0.1"));
        assert!(is_private_ipv4("172.16.0.1"));
        assert!(is_private_ipv4("172.31.255.255"));
        assert!(is_private_ipv4("192.168.1.1"));
        assert!(is_private_ipv4("127.0.0.1"));
        assert!(is_private_ipv4("169.254.10.20"));
        assert!(!is_private_ipv4("8.8.8.8"));
        assert!(!is_private_ipv4("172.32.0.1"));
        assert!(!is_private_ipv4("not-an-ip"));
    }
    #[test]
    fn test_validate_db_connection_string() {
        assert!(validate_db_connection_string("postgresql://user:pass@host:5432/db"));
        assert!(validate_db_connection_string(
            "mongodb://admin:s3cret@10.0.0.5:27017/app"
        ));
        assert!(!validate_db_connection_string("postgresql://host/db")); // no creds
        assert!(!validate_db_connection_string("postgresql://user:@host/db")); // empty password
        assert!(!validate_db_connection_string("postgresql://:pass@host/db")); // empty user
        assert!(validate_db_connection_string("postgresql://user:p@ss@host/db")); // @ in password
        assert!(!validate_db_connection_string("https://example.com/page")); // wrong scheme
    }
    #[test]
    fn test_dedupe_spans_overlap_merge() {
        let spans = vec![
            DetectedSpan {
                start: 0,
                end: 10,
                label: "person".into(),
                confidence: 0.6,
            },
            DetectedSpan {
                start: 5,
                end: 15,
                label: "full_name".into(),
                confidence: 0.8,
            },
        ];
        let out = dedupe_spans(spans);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].label, "full_name");
    }
    #[test]
    fn test_dedupe_spans_adjacent_preserved() {
        let spans = vec![
            DetectedSpan {
                start: 0,
                end: 5,
                label: "a".into(),
                confidence: 0.9,
            },
            DetectedSpan {
                start: 5,
                end: 10,
                label: "b".into(),
                confidence: 0.9,
            },
        ];
        assert_eq!(dedupe_spans(spans).len(), 2);
    }
    #[test]
    fn test_dedupe_spans_empty() {
        assert!(dedupe_spans(vec![]).is_empty());
    }
    fn test_entity(entity_id: &str) -> PiiEntity {
        PiiEntity {
            entity_id: entity_id.into(),
            category: "iban".into(),
            subcategory: None,
            value_hash: "abc123".into(),
            confidence: 0.97,
            detector_version: "rule-iban-v1".into(),
            locations: vec![],
            detected_at: 1786051200000000,
            processed_by: "test".into(),
            legal_basis: None,
            retention_until: None,
            risk_level: RiskLevel::High,
            lineage: vec![],
        }
    }
    #[test]
    fn test_soft_erase_preserves_audit_fields() {
        let mut e = test_entity("e1");
        e.soft_erase();
        assert!(e.is_erased());
        assert_eq!(e.category, "iban");
        assert_eq!(e.detected_at, 1786051200000000);
        assert!(!test_entity("e2").is_erased());
    }
}
