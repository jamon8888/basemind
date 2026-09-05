//! PII detection patterns: EU national ID checksum validation, code security regex,
//! and sensitivity tier smoke tests.

use basemind::pii::{
    CODE_SECURITY_PATTERNS, CodeSecurityPattern, EU_NATIONAL_ID_PATTERNS, EuNationalIdPattern, IBAN_REGEX, PiiCategory,
    RiskLevel, RrfWeights, SensitivityTier, suppress_by_sensitivity, validate_eu_national_id, validate_iban,
};

fn matches(label: &str, text: &str) -> bool {
    CODE_SECURITY_PATTERNS
        .iter()
        .chain(EU_NATIONAL_ID_PATTERNS.iter().map(|p| &CodeSecurityPattern {
            label: p.label,
            regex: p.regex,
            sensitivity: 0.0,
        }))
        .find(|p| p.label == label)
        .map(|p| regex::Regex::new(p.regex).map(|re| re.is_match(text)).unwrap_or(false))
        .unwrap_or(false)
}

mod eu_national_ids {
    use super::*;

    #[test]
    fn fr_nir_valid() {
        assert!(matches("national_id_fr", "18507151234667"));
        assert!(validate_eu_national_id("national_id_fr", "18507151234667"));
    }
    #[test]
    fn fr_nir_invalid_checksum() {
        assert!(matches("national_id_fr", "18507151234668"));
        assert!(!validate_eu_national_id("national_id_fr", "18507151234668"));
    }
    #[test]
    fn fr_nir_too_short() {
        assert!(!matches("national_id_fr", "1234567890123"));
    }
    #[test]
    fn nl_bsn_valid() {
        assert!(matches("national_id_nl", "190144410"));
        assert!(validate_eu_national_id("national_id_nl", "190144410"));
    }
    #[test]
    fn nl_bsn_valid_8_digit() {
        assert!(matches("national_id_nl", "14233443"));
        assert!(validate_eu_national_id("national_id_nl", "14233443"));
    }
    #[test]
    fn nl_bsn_invalid_checksum() {
        assert!(matches("national_id_nl", "123456789"));
        assert!(!validate_eu_national_id("national_id_nl", "123456789"));
    }
    #[test]
    fn be_niss_valid() {
        assert!(matches("national_id_be", "00012512345"));
        assert!(validate_eu_national_id("national_id_be", "00012512345"));
    }
    #[test]
    fn at_svnr_valid() {
        assert!(matches("national_id_at", "185302153"));
        assert!(validate_eu_national_id("national_id_at", "185302153"));
    }
    #[test]
    fn ie_pps_valid() {
        assert!(matches("national_id_ie", "1234567A"));
        assert!(validate_eu_national_id("national_id_ie", "1234567A"));
    }
    #[test]
    fn pt_nif_valid() {
        assert!(matches("national_id_pt", "299999999"));
        assert!(validate_eu_national_id("national_id_pt", "299999999"));
    }
    #[test]
    fn all_eu_patterns_have_valid_regex() {
        for pat in EU_NATIONAL_ID_PATTERNS {
            regex::Regex::new(pat.regex).expect(&format!("invalid regex for {}", pat.label));
        }
    }
}

mod iban {
    use super::*;

    #[test]
    fn iban_valid_de() {
        let re = regex::Regex::new(IBAN_REGEX).unwrap();
        assert!(re.is_match("DE89370400440532013000"));
        assert!(validate_iban("DE89370400440532013000"));
    }
    #[test]
    fn iban_valid_fr() {
        assert!(validate_iban("FR1420041010050500013M02606"));
    }
    #[test]
    fn iban_valid_nl() {
        assert!(validate_iban("NL91ABNA0417164300"));
    }
    #[test]
    fn iban_invalid_checksum() {
        assert!(!validate_iban("DE89370400440532013001"));
    }
    #[test]
    fn iban_too_short() {
        assert!(!validate_iban("DE89"));
    }
    #[test]
    fn iban_with_spaces() {
        assert!(validate_iban("DE89 3704 0044 0532 0130 00"));
    }
}

mod code_security {
    use super::*;

    fn matches(label: &str, text: &str) -> bool {
        CODE_SECURITY_PATTERNS
            .iter()
            .find(|p| p.label == label)
            .map(|p| regex::Regex::new(p.regex).map(|re| re.is_match(text)).unwrap_or(false))
            .unwrap_or(false)
    }

    #[test]
    fn aws_access_key_valid() {
        assert!(matches("aws_access_key", "AKIAIOSFODNN7EXAMPLE"));
    }
    #[test]
    fn aws_access_key_wrong_prefix() {
        assert!(!matches("aws_access_key", "AKIAAAA123456789012"));
    }
    #[test]
    fn aws_secret_key_contextual() {
        let text = "aws_secret_access_key = wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY";
        assert!(matches("aws_secret_key", text));
    }
    #[test]
    fn jwt_token_valid() {
        let token = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiaWF0IjoxNTE2MjM5MDIyfQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";
        assert!(matches("jwt_token", token));
    }
    #[test]
    fn jwt_token_partial() {
        assert!(!matches(
            "jwt_token",
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0"
        ));
    }
    #[test]
    fn bearer_token_authorization_header() {
        assert!(matches("bearer_token", "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9"));
        assert!(matches("bearer_token", "Bearer tok_1234567890abcdefghij"));
    }
    #[test]
    fn ssh_private_key_detected() {
        assert!(matches("ssh_private_key", "-----BEGIN OPENSSH PRIVATE KEY-----"));
        assert!(matches("ssh_private_key", "-----BEGIN RSA PRIVATE KEY-----"));
    }
    #[test]
    fn pgp_private_key_detected() {
        assert!(matches("gpg_private_key", "-----BEGIN PGP PRIVATE KEY BLOCK-----"));
    }
    #[test]
    fn tls_certificate_detected() {
        assert!(matches("tls_certificate", "-----BEGIN CERTIFICATE-----"));
    }
    #[test]
    fn db_connection_string_postgres() {
        assert!(matches(
            "db_connection_string",
            "postgresql://user:password@db.example.com:5432/mydb"
        ));
    }
    #[test]
    fn db_connection_string_mysql() {
        assert!(matches(
            "db_connection_string",
            "mysql://root:secret@localhost:3306/app"
        ));
    }
    #[test]
    fn db_connection_string_no_credentials() {
        assert!(!matches(
            "db_connection_string",
            "postgresql://db.example.com:5432/mydb"
        ));
    }
    #[test]
    fn env_secret_export() {
        assert!(matches("env_secret", "export AWS_SECRET_ACCESS_KEY=abc1234567890"));
        assert!(matches("env_secret", "SET API_KEY=sk_live_abcdefghijklmnop"));
    }
    #[test]
    fn env_secret_no_match_read() {
        assert!(!matches("env_secret", "echo $AWS_SECRET_ACCESS_KEY"));
        assert!(!matches("env_secret", "printenv API_KEY"));
    }
    #[test]
    fn internal_hostname_detected() {
        assert!(matches("internal_hostname", "dev-api.internal"));
        assert!(matches("internal_hostname", "db.corp.local"));
    }
    #[test]
    fn internal_hostname_public_not_matched() {
        assert!(!matches("internal_hostname", "api.example.com"));
    }
    #[test]
    fn internal_url_detected() {
        assert!(matches("internal_url", "http://192.168.1.1/api/users"));
        assert!(matches("internal_url", "https://dev-api.corp.local/v1/data"));
    }
    #[test]
    fn ipv4_private_detected() {
        assert!(matches("ipv4_private", "10.0.0.1"));
        assert!(matches("ipv4_private", "172.16.5.1"));
        assert!(matches("ipv4_private", "192.168.1.100"));
        assert!(matches("ipv4_private", "127.0.0.1"));
        assert!(matches("ipv4_private", "169.254.169.254"));
    }
    #[test]
    fn ipv4_public_not_matched() {
        assert!(!matches("ipv4_private", "8.8.8.8"));
    }
    #[test]
    fn ipv6_private_detected() {
        assert!(matches("ipv6_private", "fe80::1"));
        assert!(matches("ipv6_private", "fc00::1"));
        assert!(matches("ipv6_private", "::1"));
    }
    #[test]
    fn mac_address_detected() {
        assert!(matches("mac_address", "f8:ff:c2:00:00:00"));
    }
    #[test]
    fn all_code_security_patterns_have_valid_regex() {
        for pat in CODE_SECURITY_PATTERNS {
            regex::Regex::new(pat.regex).expect(&format!("invalid regex for {}", pat.label));
        }
    }
}

mod sensitivity_tiers {
    use super::*;

    #[test]
    fn hard_block_tiers() {
        assert_eq!(f32::from(&PiiCategory::AwsAccessKey), SensitivityTier::HARD_BLOCK);
        assert_eq!(f32::from(&PiiCategory::JwtToken), SensitivityTier::HARD_BLOCK);
        assert_eq!(f32::from(&PiiCategory::SshPrivateKey), SensitivityTier::HARD_BLOCK);
        assert_eq!(f32::from(&PiiCategory::DbConnectionString), SensitivityTier::HARD_BLOCK);
        assert_eq!(f32::from(&PiiCategory::EnvSecret), SensitivityTier::HARD_BLOCK);
    }
    #[test]
    fn high_tiers() {
        assert_eq!(f32::from(&PiiCategory::NationalIdFr), SensitivityTier::HIGH);
        assert_eq!(f32::from(&PiiCategory::NationalIdNl), SensitivityTier::HIGH);
        assert_eq!(f32::from(&PiiCategory::Iban), SensitivityTier::HIGH);
        assert_eq!(f32::from(&PiiCategory::CreditCard), SensitivityTier::HIGH);
    }
    #[test]
    fn medium_tiers() {
        assert_eq!(f32::from(&PiiCategory::InternalUrl), SensitivityTier::MEDIUM);
        assert_eq!(f32::from(&PiiCategory::InternalHostname), SensitivityTier::MEDIUM);
    }
    #[test]
    fn low_tiers() {
        assert_eq!(f32::from(&PiiCategory::Email), SensitivityTier::LOW);
        assert_eq!(f32::from(&PiiCategory::PhoneNumber), SensitivityTier::LOW);
    }
    #[test]
    fn minimal_tiers() {
        assert_eq!(f32::from(&PiiCategory::PersonFullName), SensitivityTier::MINIMAL);
        assert_eq!(f32::from(&PiiCategory::Organization), 0.2);
        assert_eq!(f32::from(&PiiCategory::Location), 0.2);
    }
    #[test]
    fn risk_level_from_sensitivity() {
        assert_eq!(RiskLevel::from(SensitivityTier::HARD_BLOCK), RiskLevel::Critical);
        assert_eq!(RiskLevel::from(SensitivityTier::HIGH), RiskLevel::High);
        assert_eq!(RiskLevel::from(SensitivityTier::MEDIUM), RiskLevel::Medium);
        assert_eq!(RiskLevel::from(SensitivityTier::LOW), RiskLevel::Low);
    }
}

mod rrf_weights {
    use super::*;

    #[test]
    fn pii_workload_weights() {
        let w = RrfWeights::PII_WORKLOAD;
        assert_eq!(w.exact, 3.0);
        assert_eq!(w.keyword, 2.0);
        assert_eq!(w.vector, 1.0);
    }
    #[test]
    fn balanced_weights() {
        let w = RrfWeights::BALANCED;
        assert_eq!(w.exact, 1.0);
        assert_eq!(w.keyword, 1.0);
        assert_eq!(w.vector, 1.0);
    }
}

mod suppression {
    use super::*;

    #[test]
    fn hard_block_suppresses_to_zero() {
        let mut results = vec![("chunk_a".to_string(), 0.9), ("chunk_b".to_string(), 0.8)];
        let sensitivities = vec![("chunk_a".to_string(), SensitivityTier::HARD_BLOCK)];
        suppress_by_sensitivity(&mut results, &sensitivities);
        assert!((results[0].1 - 0.09).abs() < 1e-6);
    }
    #[test]
    fn no_sensitivity_leaves_score_unchanged() {
        let mut results = vec![("chunk_a".to_string(), 0.9)];
        suppress_by_sensitivity(&mut results, &[]);
        assert!((results[0].1 - 0.9).abs() < 1e-6);
    }
    #[test]
    fn high_sensitivity_deweights() {
        let mut results = vec![("chunk_a".to_string(), 1.0)];
        let sensitivities = vec![("chunk_a".to_string(), SensitivityTier::HIGH)];
        suppress_by_sensitivity(&mut results, &sensitivities);
        assert!((results[0].1 - 0.19).abs() < 1e-6);
    }
}

mod category_labels {
    use super::*;

    #[test]
    fn eu_national_id_labels_present() {
        let labels = PiiCategory::eu_national_id_labels();
        assert!(labels.contains(&"national_id_fr"));
        assert!(labels.contains(&"national_id_nl"));
        assert!(labels.contains(&"national_id_be"));
        assert!(labels.contains(&"national_id_at"));
        assert!(labels.contains(&"national_id_ie"));
        assert!(labels.contains(&"national_id_pt"));
        assert_eq!(labels.len(), 7);
    }
    #[test]
    fn code_security_labels_present() {
        let labels = PiiCategory::code_security_labels();
        assert!(labels.contains(&"aws_access_key"));
        assert!(labels.contains(&"jwt_token"));
        assert!(labels.contains(&"ssh_private_key"));
        assert!(labels.contains(&"env_secret"));
        assert!(labels.contains(&"internal_url"));
    }
}
