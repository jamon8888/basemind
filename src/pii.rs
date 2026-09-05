//! PII detection patterns for GDPR-compliant EU document processing.

use serde::{Deserialize, Serialize};

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

// ─── EU National ID validators ────────────────────────────────────────────────

/// Validates a French NIR (Numéro d'Inscription au Répertoire).
/// Returns true if the 15-digit number passes the MOD-97 checksum.
pub fn validate_fr_nir(s: &str) -> bool {
    let digits: Vec<u32> = s.chars().filter_map(|c| c.to_digit(10)).collect();
    if digits.len() != 15 {
        return false;
    }
    let first13: String = digits.iter().take(13).map(|d| d.to_string()).collect();
    let first13_val: u64 = first13.parse().unwrap_or(u64::MAX);
    let remainder = first13_val % 97;
    let check = (97 - remainder as u32) % 97;
    let last2 = digits[13] * 10 + digits[14];
    last2 == check
}

/// Validates a Dutch BSN (Burgerservicenummer) using the elf-proef (11-check).
/// Accepts 8-digit (padded with leading zero) or 9-digit numbers.
pub fn validate_nl_bsn(s: &str) -> bool {
    let digits: Vec<u32> = s.chars().filter_map(|c| c.to_digit(10)).collect();
    if digits.len() == 9 {
        bsn_eleven_proef(&digits)
    } else if digits.len() == 8 {
        bsn_eleven_proef(&[0].iter().chain(digits.iter()).cloned().collect::<Vec<_>>()[..])
    } else {
        false
    }
}

fn bsn_eleven_proef(digits: &[u32]) -> bool {
    let sum: i32 = digits
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let w = if i == digits.len() - 1 { -1 } else { 9 - i as i32 };
            *d as i32 * w
        })
        .sum();
    sum % 11 == 0
}

/// Validates a Belgian NISS (Numéro d'Identification de la Sécurité Sociale).
/// Returns true if the 11-digit number passes the MOD-97 + check-digit formula.
pub fn validate_be_niss(s: &str) -> bool {
    let digits: Vec<u32> = s.chars().filter_map(|c| c.to_digit(10)).collect();
    if digits.len() != 11 {
        return false;
    }
    let first9: String = digits.iter().take(9).map(|d| d.to_string()).collect();
    let first9_val: u64 = first9.parse().unwrap_or(u64::MAX);
    let remainder = first9_val % 97;
    let last2 = digits[9] * 10 + digits[10];
    let expected = ((97 - remainder as u32) * 100) % 97;
    last2 == expected
}

/// Validates an Austrian SVNR (Sozialversicherungsnummer).
/// Returns true if the 10-digit number passes the weighted MOD-11 checksum.
/// The 4th digit is the check digit; weights [3,7,9,5,8,4,2,1,6] are applied
/// to the remaining 9 digits (positions 1-3 and 5-10), and the 10th position
/// (index 3) must equal sum % 11.
pub fn validate_at_svnr(s: &str) -> bool {
    let digits: Vec<u32> = s.chars().filter_map(|c| c.to_digit(10)).collect();
    if digits.len() != 10 {
        return false;
    }
    let check = digits[3];
    let sum = digits[0] * 3
        + digits[1] * 7
        + digits[2] * 9
        + digits[4] * 5
        + digits[5] * 8
        + digits[6] * 4
        + digits[7] * 2
        + digits[8]
        + digits[9] * 6;
    (sum % 11) == check
}

/// Validates an Irish PPS (Personal Public Service number).
/// Returns true if the 8-character string passes the weighted MOD-23 checksum.
pub fn validate_ie_pps(s: &str) -> bool {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() != 8 {
        return false;
    }
    let digits: Vec<u32> = chars[0..7].iter().filter_map(|c| c.to_digit(10)).collect();
    if digits.len() != 7 {
        return false;
    }
    let weights: [u32; 7] = [8, 7, 6, 5, 4, 3, 2];
    let sum: u32 = digits.iter().zip(weights.iter()).map(|(d, w)| d * w).sum();
    let remainder = sum % 23;
    let check_char = match remainder {
        0..=21 => (b'A' + remainder as u8) as char,
        _ => return false,
    };
    chars[7] == check_char
}

/// Validates a Portuguese NIF (Número de Identificação Fiscal).
/// Returns true if the 9-digit number passes the weighted MOD-11 checksum.
pub fn validate_pt_nif(s: &str) -> bool {
    let digits: Vec<u32> = s.chars().filter_map(|c| c.to_digit(10)).collect();
    if digits.len() != 9 {
        return false;
    }
    let sum = digits[0] * 9
        + digits[1] * 8
        + digits[2] * 7
        + digits[3] * 6
        + digits[4] * 5
        + digits[5] * 4
        + digits[6] * 3
        + digits[7] * 2;
    let remainder = sum % 11;
    let expected = if remainder < 2 { 0 } else { 11 - remainder };
    expected == digits[8]
}

/// Dispatches to the country-specific validator for EU national IDs.
/// Returns true for any label not in the known set (idempotent pass-through).
pub fn validate_eu_national_id(label: &str, value: &str) -> bool {
    match label {
        "national_id_fr" => validate_fr_nir(value),
        "national_id_nl" => validate_nl_bsn(value),
        "national_id_be" => validate_be_niss(value),
        "national_id_at" => validate_at_svnr(value),
        "national_id_ie" => validate_ie_pps(value),
        "national_id_pt" => validate_pt_nif(value),
        _ => true,
    }
}

pub struct EuNationalIdPattern {
    pub label: &'static str,
    pub country: &'static str,
    pub regex: &'static str,
}

pub const EU_NATIONAL_ID_PATTERNS: &[EuNationalIdPattern] = &[
    EuNationalIdPattern {
        label: "national_id_fr",
        country: "FR",
        regex: r"\b[12]\d{2}(?:0[1-9]|1[0-2])\d{10}\b",
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
        regex: r"(?i)^(?:[fF][cCdD][0-9a-fA-F]{2}:[0-9a-fA-F:]{1,39}|fe80:[0-9a-fA-F:]{1,39}|::1|0:0:0:0:0:0:0:1)$",
        sensitivity: SensitivityTier::MEDIUM,
    },
    CodeSecurityPattern {
        label: "internal_hostname",
        regex: r"(?i)\.(?:internal|corp)$",
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

/// Valid IBAN lengths per country code (ISO 13616 IBAN Registry).
/// Covers all EU/EEA member states and common international codes.
///ponytail: basic HashMap lookup — replace with IBAN registry crate if per-country format validation is needed
static IBAN_LENGTHS: std::sync::LazyLock<std::collections::HashMap<&'static str, usize>> =
    std::sync::LazyLock::new(|| {
        let mut m = std::collections::HashMap::new();
        // EU/EEA
        m.insert("AT", 20); // Austria
        m.insert("BE", 16); // Belgium
        m.insert("BG", 22); // Bulgaria
        m.insert("HR", 21); // Croatia
        m.insert("CY", 28); // Cyprus
        m.insert("CZ", 24); // Czech Republic
        m.insert("DK", 18); // Denmark (incl. FO, GL)
        m.insert("EE", 20); // Estonia
        m.insert("FI", 18); // Finland (incl. AX)
        m.insert("FR", 27); // France (incl. GF, GP, MQ, RE, YT, NC, PF, TF, BL, MF, PM, WF)
        m.insert("DE", 22); // Germany
        m.insert("GR", 27); // Greece
        m.insert("HU", 28); // Hungary
        m.insert("IE", 22); // Ireland
        m.insert("IT", 27); // Italy (incl. SM, VA)
        m.insert("LV", 21); // Latvia
        m.insert("LI", 21); // Liechtenstein
        m.insert("LT", 20); // Lithuania
        m.insert("LU", 20); // Luxembourg
        m.insert("MT", 31); // Malta
        m.insert("MC", 27); // Monaco
        m.insert("NL", 18); // Netherlands (incl. AW, CW, SX)
        m.insert("NO", 15); // Norway (incl. SJ, BV)
        m.insert("PL", 28); // Poland
        m.insert("PT", 25); // Portugal (incl. MH, PW)
        m.insert("RO", 24); // Romania
        m.insert("SM", 27); // San Marino
        m.insert("SK", 24); // Slovakia
        m.insert("SI", 19); // Slovenia
        m.insert("ES", 24); // Spain (incl. AD)
        m.insert("SE", 24); // Sweden
        m.insert("CH", 21); // Switzerland (incl. VA)
        m.insert("GB", 22); // United Kingdom (incl. IM, JE, GG)
        m
    });

/// Validates an IBAN using the MOD-97 checksum (ISO 13616).
/// Returns true if the country code is registered, the length is correct, and the check digits are valid.
pub fn validate_iban(iban: &str) -> bool {
    let iban = iban.chars().filter(|c| !c.is_whitespace()).collect::<String>();
    if !(5..=34).contains(&iban.len()) {
        return false;
    }
    if !iban.chars().all(|c| c.is_ascii_alphanumeric()) {
        return false;
    }
    let country = &iban[..2];
    if !iban.chars().take(2).all(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    if !iban.chars().skip(2).take(2).all(|c| c.is_ascii_digit()) {
        return false;
    }
    if let Some(&expected_len) = IBAN_LENGTHS.get(country) {
        if iban.len() != expected_len {
            return false;
        }
    } else {
        return false;
    }
    let normalized = iban.to_ascii_uppercase();
    let acc = normalized
        .as_bytes()
        .iter()
        .cycle()
        .skip(4)
        .take(normalized.len())
        .fold(0_u64, |acc, &c| {
            if c.is_ascii_digit() {
                (acc * 10 + u64::from(c - b'0')) % 97
            } else {
                (acc * 100 + u64::from(c - b'A' + 10)) % 97
            }
        });
    acc == 1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityLocation {
    pub file_id: String,
    pub chunk_index: i32,
    pub char_start: i32,
    pub char_end: i32,
    pub page_number: Option<i32>,
    pub context: String,
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
    /// RFC 3339 timestamp when the entity was first detected.
    pub detected_at: String,
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
        assert!(validate_be_niss("00012512321"));
    }
    #[test]
    fn test_at_svnr_valid() {
        assert!(validate_at_svnr("7829280755"));
    }
    #[test]
    fn test_ie_pps_valid() {
        assert!(validate_ie_pps("1234567U"));
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
}
