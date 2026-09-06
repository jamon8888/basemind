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
/// Validates a French NIR (Numéro d'Inscription au Répertoire).
/// 15 chars; Corsica department codes `2A`/`2B` at positions 6–7 are
/// substituted with `19`/`18` before the MOD-97 checksum (per python-stdnum
/// `fr.nir`). A zero remainder yields check digits `97`, not `00`.
pub fn validate_fr_nir(s: &str) -> bool {
    let s: String = s
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_uppercase();
    let chars: Vec<char> = s.chars().collect();
    if chars.len() != 15 || !chars[..5].iter().all(|c| c.is_ascii_digit()) {
        return false;
    }
    let dept: String = chars[5..7].iter().collect();
    let dept = match dept.as_str() {
        "2A" => "19",
        "2B" => "18",
        d if d.chars().all(|c| c.is_ascii_digit()) => {
            if !chars[7..13].iter().all(|c| c.is_ascii_digit()) || !chars[13..15].iter().all(|c| c.is_ascii_digit()) {
                return false;
            }
            d
        }
        _ => return false,
    };
    let first13: String = chars[..5].iter().collect::<String>() + dept + &chars[7..13].iter().collect::<String>();
    let first13_val: u64 = first13.parse().unwrap_or(u64::MAX);
    if first13_val == u64::MAX {
        return false;
    }
    let check = 97 - (first13_val % 97) as u32;
    let last2: u32 = chars[13..15].iter().collect::<String>().parse().unwrap_or(u32::MAX);
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

/// Validates a Belgian NISS (Numéro d'Identification de la Sécurité Sociale,
/// Rijksregisternummer). 11 digits: YYMMDD + 3-digit serial + 2-digit check.
/// Checksum per python-stdnum `be.nn`: `97 - (int(n[:-2]) % 97) == int(n[-2:])`,
/// trying both the bare number and the `2`-prefixed variant for post-2000 births.
/// No birth-date validation — the regex is conservative, the checksum filters.
pub fn validate_be_niss(s: &str) -> bool {
    let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() != 11 || digits.chars().all(|c| c == '0') {
        return false;
    }
    let be_check = |n: &str| -> bool {
        let (head, tail) = n.split_at(n.len() - 2);
        let head_val: u64 = head.parse().unwrap_or(u64::MAX);
        let tail_val: u32 = tail.parse().unwrap_or(u32::MAX);
        let check = 97 - (head_val % 97) as u32;
        check == tail_val
    };
    if be_check(&digits) {
        return true;
    }
    // Post-2000 births: the same check against '2' + number (stdnum tries this
    // variant when YY + 2000 <= current year; trying unconditionally is a
    // negligible over-accept for a checksum pre-filter).
    be_check(&format!("2{digits}"))
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
/// 7 digits + check letter (8 chars), optionally followed by a second letter
/// (9 chars). Check digit per python-stdnum `ie.vat.calc_check_digit`:
/// alphabet `WABCDEFGHIJKLMNOPQRSTUV`, weights 8..2 over the 7 digits plus
/// `9 * alphabet.index(second_letter)` for 2013-format numbers whose second
/// letter is in `ABH`. Old-format second letters (`WTX`) are ignored.
pub fn validate_ie_pps(s: &str) -> bool {
    const ALPHABET: &str = "WABCDEFGHIJKLMNOPQRSTUV";
    let s: String = s
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect::<String>()
        .to_ascii_uppercase();
    let chars: Vec<char> = s.chars().collect();
    if chars.len() != 8 && chars.len() != 9 {
        return false;
    }
    if !chars[..7].iter().all(|c| c.is_ascii_digit()) {
        return false;
    }
    // First check letter is restricted to A-W (stdnum `pps_re`).
    if !(('A'..='W').contains(&chars[7])) {
        return false;
    }
    let digits: Vec<u32> = chars[..7].iter().filter_map(|c| c.to_digit(10)).collect();
    let mut sum: u32 = digits.iter().enumerate().map(|(i, d)| d * (8 - i as u32)).sum();
    if chars.len() == 9 {
        let second = chars[8];
        if "ABH".contains(second) {
            let idx = ALPHABET.find(second).unwrap_or(0) as u32;
            sum += 9 * idx;
        } else if !"WTX".contains(second) {
            return false;
        }
        // Old-format (WTX): second letter ignored, falls through to 7-digit check.
    }
    let expected = ALPHABET.chars().nth((sum % 23) as usize).unwrap_or('?');
    chars[7] == expected
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
        m.insert("IS", 26); // Iceland
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
        m.insert("ES", 24); // Spain
        m.insert("SE", 24); // Sweden
        m.insert("CH", 21); // Switzerland
        m.insert("GB", 22); // United Kingdom (incl. IM, JE, GG)
        m.insert("AD", 24); // Andorra
        m.insert("AL", 28); // Albania
        m.insert("MK", 19); // North Macedonia
        m.insert("XK", 20); // Kosovo
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

// ─── Pipeline helpers: thresholds, span dedup, format validators, erasure ───

/// Per-entity-type confidence threshold for the Layer-4 pipeline filter (spec §3).
/// High legal/financial risk and secrets sit at 0.95; contact identifiers lower.
/// Unknown categories default to 0.80.
pub fn confidence_threshold(category: &str) -> f32 {
    match category {
        "national_id"
        | "national_id_fr"
        | "national_id_nl"
        | "national_id_be"
        | "national_id_at"
        | "national_id_ie"
        | "national_id_pt"
        | "national_id_generic"
        | "iban"
        | "credit_card"
        | "health_data"
        | "biometric"
        | "genetic"
        | "api_key"
        | "aws_access_key"
        | "aws_secret_key"
        | "gcp_credentials"
        | "azure_credentials"
        | "jwt_token"
        | "oauth_token"
        | "bearer_token"
        | "ssh_private_key"
        | "gpg_private_key"
        | "tls_certificate"
        | "db_connection_string"
        | "env_secret" => 0.95,
        "internal_hostname" | "internal_url" | "email" => 0.90,
        "phone" | "phone_number" | "ip_address" | "ipv4" | "ipv6" | "ipv4_private" | "ipv6_private" => 0.85,
        "person_name" | "person" | "full_name" | "first_name" | "last_name" => 0.75,
        "organization" | "location" => 0.70,
        _ => 0.80,
    }
}

/// Returns true when a detection at `confidence` survives the Layer-4 filter.
pub fn passes_threshold(category: &str, confidence: f32) -> bool {
    confidence >= confidence_threshold(category)
}

/// Raw GLiNER label thresholds (spec §17). Lenient for secrets (high recall —
/// format validation downstream removes false positives), strict for names
/// (the model over-predicts `person`/`full_name`). Unknown labels default to 0.5.
pub fn gliner_label_threshold(label: &str) -> f32 {
    match label {
        "full_name" | "person" => 0.7,
        "api_key" | "password" | "iban" | "ip_address" => 0.3,
        _ => 0.5,
    }
}

/// Validates a phone number against E.164 (spec user story 7): leading `+`,
/// 8–15 digits, no leading zero after `+`. Formatting characters
/// (spaces, dashes, parens, dots) are stripped before validation.
pub fn validate_e164(phone: &str) -> bool {
    let normalized: String = phone
        .chars()
        .filter(|c| !matches!(c, ' ' | '-' | '(' | ')' | '.'))
        .collect();
    let digits = normalized.strip_prefix('+').unwrap_or("");
    if normalized.len() != digits.len() + 1 {
        return false;
    }
    if !(8..=15).contains(&digits.len()) {
        return false;
    }
    if !digits.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    !digits.starts_with('0')
}

fn is_base64url_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '='
}

/// Returns true when `=` padding appears only as trailing padding (≤2 chars).
fn has_valid_padding(part: &str) -> bool {
    let stripped = part.trim_end_matches('=');
    !stripped.contains('=') && part.len() - stripped.len() <= 2
}

/// Validates JWT structure (spec §16): three non-empty dot-separated
/// base64url sections, header starting with `eyJ`.
pub fn validate_jwt(token: &str) -> bool {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 || parts.iter().any(|p| p.is_empty()) {
        return false;
    }
    if !parts[0].starts_with("eyJ") {
        return false;
    }
    parts
        .iter()
        .all(|p| p.chars().all(is_base64url_char) && has_valid_padding(p))
}

/// Returns true for RFC 1918 private ranges (10/8, 172.16/12, 192.168/16)
/// plus loopback (127/8) and link-local (169.254/16).
pub fn is_private_ipv4(ip: &str) -> bool {
    let octets: Vec<u32> = ip.split('.').filter_map(|o| o.parse().ok()).collect();
    if octets.len() != 4 || octets.iter().any(|o| *o > 255) {
        return false;
    }
    let [a, b, ..] = octets[..] else { return false };
    a == 10 || (a == 172 && (16..=31).contains(&b)) || (a == 192 && b == 168) || a == 127 || (a == 169 && b == 254)
}

/// Validates a DB connection string (spec §16): known scheme, `://` authority
/// with `user:password@host` (non-empty password). Rejects credential-less
/// URIs like `postgresql://host/db`.
pub fn validate_db_connection_string(uri: &str) -> bool {
    let lower = uri.to_ascii_lowercase();
    let scheme_ok = [
        "postgresql://",
        "postgres://",
        "mysql://",
        "mongodb://",
        "jdbc:",
        "mssql://",
        "redis://",
    ]
    .iter()
    .any(|p| lower.contains(p));
    if !scheme_ok {
        return false;
    }
    let Some(auth) = uri.split("://").nth(1) else {
        return false;
    };
    let Some((userinfo, host)) = auth.rsplit_once('@') else {
        return false;
    };
    let Some((user, password)) = userinfo.split_once(':') else {
        return false;
    };
    !user.is_empty() && !password.is_empty() && !host.is_empty()
}

/// One PII detection span. `start`/`end` are char offsets into the chunk text.
#[derive(Debug, Clone, PartialEq)]
pub struct DetectedSpan {
    pub start: usize,
    pub end: usize,
    pub label: String,
    pub confidence: f32,
}

/// Merges overlapping detection spans (spec user story 8) so GLiNER and regex
/// hits on the same mention collapse to a single entity. The higher-confidence
/// span wins; ties keep the longer span. Adjacent (non-overlapping) spans are
/// preserved. Output is sorted by `start`.
pub fn dedupe_spans(mut spans: Vec<DetectedSpan>) -> Vec<DetectedSpan> {
    spans.sort_by_key(|s| (s.start, s.end));
    let mut out: Vec<DetectedSpan> = Vec::with_capacity(spans.len());
    for span in spans {
        if let Some(last) = out.last_mut()
            && span.start < last.end
        {
            if span.confidence > last.confidence
                || (span.confidence == last.confidence && (span.end - span.start) > (last.end - last.start))
            {
                *last = span;
            } else {
                last.end = last.end.max(span.end);
            }
            continue;
        }
        out.push(span);
    }
    out
}

/// Marker written into `value_hash` by [`PiiEntity::soft_erase`].
pub const ERASED_VALUE_HASH: &str = "ERASED";

impl PiiEntity {
    /// Right-to-erasure soft erase (spec §9 step 3): drops the value hash while
    /// preserving `category`, `locations` and `detected_at` for audit.
    /// The tombstone can never collide with a real entry: `value_hash` holds a
    /// hex SHA-256 digest, which never equals `"ERASED"`.
    pub fn soft_erase(&mut self) {
        self.value_hash = ERASED_VALUE_HASH.to_string();
    }

    /// Returns true after [`PiiEntity::soft_erase`] ran.
    pub fn is_erased(&self) -> bool {
        self.value_hash == ERASED_VALUE_HASH
    }
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
            detected_at: "2026-09-06T00:00:00Z".into(),
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
        assert_eq!(e.detected_at, "2026-09-06T00:00:00Z");
        assert!(!test_entity("e2").is_erased());
    }
}
