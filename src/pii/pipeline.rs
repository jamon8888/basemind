//! Layer-4+ pipeline helpers: confidence thresholds, span dedup, erasure.

use super::{PiiEntity, RiskLevel};

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

/// String-keyed risk tier mirroring the `PiiCategory` sensitivity table for
/// labels arriving as text (redaction findings, GLiNER tags). Secrets are
/// critical, national/financial identifiers high, infrastructure and contact
/// identifiers medium or low. Unknown labels default to low.
pub fn risk_for_label(label: &str) -> RiskLevel {
    match label {
        "api_key"
        | "aws_access_key"
        | "aws_secret_key"
        | "gcp_credentials"
        | "azure_credentials"
        | "jwt_token"
        | "oauth_token"
        | "oauth"
        | "bearer_token"
        | "bearer"
        | "ssh_private_key"
        | "gpg_private_key"
        | "tls_certificate"
        | "db_connection_string"
        | "env_secret"
        | "password" => RiskLevel::Critical,
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
        | "bank_account"
        | "passport_number"
        | "passport"
        | "drivers_license"
        | "tax_id"
        | "health_data"
        | "biometric"
        | "genetic" => RiskLevel::High,
        "internal_url" | "internal_hostname" | "phone" | "phone_number" | "email" => RiskLevel::Medium,
        _ => RiskLevel::Low,
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
        for loc in &mut self.locations {
            loc.context.clear();
        }
    }

    /// Returns true after [`PiiEntity::soft_erase`] ran.
    pub fn is_erased(&self) -> bool {
        self.value_hash == ERASED_VALUE_HASH
    }
}
