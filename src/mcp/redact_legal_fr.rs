//! French legal redaction policy shared by the model and pattern passes.

use std::sync::OnceLock;

/// Decide whether a high-recall engine finding is safe to discard in this legal context.
pub(super) fn is_false_positive_pattern(
    original: &str,
    finding: &xberg::types::redaction::RedactionFinding,
    legal_fr_filters: bool,
    legal_fr_v2: bool,
    preserve_document_dates: bool,
) -> bool {
    let start = finding.start as usize;
    let end = finding.end as usize;
    let Some(value) = original.get(start..end) else {
        return false;
    };
    match &finding.category {
        xberg::types::redaction::PiiCategory::Custom(label)
            if legal_fr_v2
                && !preserve_document_dates
                && matches!(label.as_str(), "date_of_birth" | "document_date") =>
        {
            // NER custom patterns are global, so remove only the occurrence
            // that is clearly an act date after xberg has matched it.
            is_act_date_context(original, start, end)
        }
        xberg::types::redaction::PiiCategory::Phone => {
            let reference_shape = if legal_fr_v2 {
                is_legal_reference_shape(value)
            } else {
                is_legacy_legal_reference_shape(value)
            };
            legal_fr_filters && reference_shape && has_reference_or_date_cue(original, start, end)
        }
        xberg::types::redaction::PiiCategory::SwiftBic => {
            // All-letter BIC-shaped words need a nearby banking cue. This
            // avoids masking ordinary uppercase prose such as ATTESTATION.
            legal_fr_filters && value.chars().all(|c| c.is_ascii_alphabetic()) && !has_bic_context(original, start, end)
        }
        _ => false,
    }
}

/// The original legal-fr@1 filter: exactly eight digits, optionally separated by hyphens.
fn is_legacy_legal_reference_shape(value: &str) -> bool {
    value.chars().filter(|c| c.is_ascii_digit()).count() == 8 && value.chars().all(|c| c.is_ascii_digit() || c == '-')
}

/// Legal-fr@2 also recognizes year references such as `2026-221`.
fn is_legal_reference_shape(value: &str) -> bool {
    let year_reference = value.split_once('-').is_some_and(|(year, suffix)| {
        year.len() == 4
            && matches!(year.as_bytes().first(), Some(b'1' | b'2'))
            && year.chars().all(|c| c.is_ascii_digit())
            && suffix.len() == 3
            && suffix.chars().all(|c| c.is_ascii_digit())
    });
    is_legacy_legal_reference_shape(value) || year_reference
}

/// Decide whether a `legal-fr@2` model span is a known false positive.
pub(super) fn should_skip_legal_fr_v2_ner_span(text: &str, start: usize, end: usize, label: &str) -> bool {
    should_skip_legal_fr_v2_ner_span_with_categories(text, start, end, label, false)
}

pub(super) fn should_skip_legal_fr_v2_ner_span_with_categories(
    text: &str,
    start: usize,
    end: usize,
    label: &str,
    preserve_document_dates: bool,
) -> bool {
    let Some(value) = text.get(start..end) else {
        return false;
    };
    match label {
        "named_place" | "city" | "state_or_region" | "country" | "location" => is_broad_geography(value),
        "organization" => {
            let normalized = normalize_ner_value(value);
            GENERIC_LEGAL_ORGANIZATIONS.contains(&normalized.as_str())
        }
        "iban" => !is_plausible_iban(value),
        "date_of_birth" | "document_date" => !preserve_document_dates && is_act_date_context(text, start, end),
        _ => false,
    }
}

/// Test-facing shorthand for a span whose value is the complete input.
#[cfg(test)]
pub(super) fn should_skip_legal_fr_v2_ner_value(value: &str, label: &str) -> bool {
    should_skip_legal_fr_v2_ner_span(value, 0, value.len(), label)
}

const GENERIC_LEGAL_ORGANIZATIONS: &[&str] = &["sécurité sociale", "ma société", "siège social", "employeur"];

/// Geography that describes a broad legal scope, not a private named place.
fn is_broad_geography(value: &str) -> bool {
    const BROAD_GEOGRAPHY: &[&str] = &[
        "europe",
        "union européenne",
        "ue",
        "france métropolitaine",
        "france entière",
        "grand sud-ouest",
        "grand est",
        "grand ouest",
        "sud-ouest",
        "sud-est",
        "nord-ouest",
        "nord-est",
        "france",
        "allemagne",
        "angleterre",
        "belgique",
        "espagne",
        "italie",
        "pays-bas",
        "portugal",
        "suisse",
        "royaume-uni",
        "afrique",
        "asie",
        "amérique",
        "océanie",
        "île-de-france",
        "ile-de-france",
        "bretagne",
        "normandie",
        "occitanie",
        "nouvelle-aquitaine",
        "provence-alpes-côte-d'azur",
        "auvergne-rhône-alpes",
        "corse",
        "espace schengen",
        "états membres",
        "pays membres",
    ];
    // Keep this an exact allowlist: broad scope terms are readable, while a
    // private place that merely contains one of those words remains redacted.
    let normalized = normalize_geography_value(value);
    BROAD_GEOGRAPHY.contains(&normalized.as_str())
        || (normalized.starts_with("grand ") && normalized.contains("région"))
}

/// Collapse whitespace and case-fold a model value for exact legal-term matching.
fn normalize_ner_value(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// Normalize punctuation and leading French geography modifiers before lookup.
fn normalize_geography_value(value: &str) -> String {
    let cleaned = value
        .to_lowercase()
        .replace('’', "'")
        .replace("l'", "l ")
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c.is_whitespace() || c == '-' || c == '\'' {
                c
            } else {
                ' '
            }
        })
        .collect::<String>();
    let words: Vec<&str> = cleaned.split_whitespace().collect();
    let first = words
        .iter()
        .take_while(|word| {
            matches!(
                **word,
                "a" | "à" | "au" | "aux" | "dans" | "de" | "des" | "du" | "en" | "l" | "la" | "le" | "les"
            )
        })
        .count();
    words[first..].join(" ")
}

/// Keep malformed but account-shaped IBAN findings redacted for safety.
fn is_plausible_iban(value: &str) -> bool {
    let compact = value
        .trim_matches(|c: char| !c.is_ascii_alphanumeric())
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>();
    let mut chars = compact.chars();
    let (Some(first), Some(second)) = (chars.next(), chars.next()) else {
        return false;
    };
    first.is_ascii_alphabetic()
        && second.is_ascii_alphabetic()
        // This is deliberately looser than IBAN validation: a malformed
        // account-shaped NER span must still be redacted, not leaked.
        && chars.filter(|c| c.is_ascii_digit()).count() >= 2
}

/// Return whether the surrounding text explicitly describes a birth date.
fn has_birth_context(text: &str, start: usize, end: usize) -> bool {
    [
        "date de naissance",
        "lieu de naissance",
        "né le",
        "née le",
        "né(e)",
        "naissance",
        "birth",
        "born",
    ]
    .iter()
    .any(|cue| has_cue_near_span(text, start, end, cue))
}

/// Return whether a date is being used as an act/document date rather than a DOB.
fn is_act_date_context(text: &str, start: usize, end: usize) -> bool {
    if has_birth_context(text, start, end) {
        return false;
    }
    [
        "depuis le",
        "à compter du",
        "fait à",
        "en date du",
        "date de l'acte",
        "date de l’acte",
        "acte du",
        "signé le",
        "conclu le",
        "entrée en vigueur",
    ]
    .iter()
    .any(|cue| has_cue_near_span(text, start, end, cue))
}

/// Match a cue at the candidate occurrence, not merely somewhere in its
/// bounded context window. This prevents one date from borrowing another
/// date's birth/act label in the same paragraph.
fn has_cue_near_span(text: &str, start: usize, end: usize, cue: &str) -> bool {
    // This is a byte bound, so leave room for multi-byte French characters.
    const MAX_CUE_DISTANCE: usize = 32;
    let (context_start, context_end) = aligned_context_bounds(text, start, end);
    let context = text.get(context_start..context_end).unwrap_or_default();
    context.char_indices().any(|(relative_start, _)| {
        let Some(cue_len) = case_insensitive_match_len(context, relative_start, cue) else {
            return false;
        };
        let cue_start = context_start + relative_start;
        let cue_end = cue_start + cue_len;
        let before = context[..relative_start].chars().next_back();
        let after = context[relative_start + cue_len..].chars().next();
        if before.is_some_and(is_word_char) || after.is_some_and(is_word_char) {
            return false;
        }
        (cue_start < end && cue_end > start)
            || (cue_end <= start && start - cue_end <= MAX_CUE_DISTANCE)
            || (cue_start >= end && cue_start - end <= MAX_CUE_DISTANCE)
    })
}

/// Return the original byte length of a case-insensitive cue at a UTF-8 boundary.
/// Keeping the match on the original text avoids offsets drifting when Unicode
/// lowercasing expands a character.
fn case_insensitive_match_len(text: &str, start: usize, cue: &str) -> Option<usize> {
    let mut chars = text[start..].char_indices();
    let mut end = start;
    for _ in cue.chars() {
        let (offset, character) = chars.next()?;
        end = start + offset + character.len_utf8();
    }
    (text.get(start..end)?.to_lowercase() == cue).then_some(end - start)
}

/// Read the bounded, UTF-8-aligned context around a model span.
fn context_lowercase(text: &str, start: usize, end: usize) -> String {
    let (context_start, context_end) = aligned_context_bounds(text, start, end);
    text.get(context_start..context_end)
        .map(str::to_lowercase)
        .unwrap_or_default()
}

/// Expand a byte window to valid UTF-8 boundaries before it is sliced.
fn aligned_context_bounds(text: &str, start: usize, end: usize) -> (usize, usize) {
    let mut context_start = start.saturating_sub(80).min(text.len());
    while context_start > 0 && !text.is_char_boundary(context_start) {
        context_start -= 1;
    }
    let mut context_end = end.saturating_add(80).min(text.len());
    while context_end < text.len() && !text.is_char_boundary(context_end) {
        context_end += 1;
    }
    (context_start, context_end)
}

/// Detect the nearby reference/date cues that distinguish references from phones.
fn has_reference_or_date_cue(text: &str, start: usize, end: usize) -> bool {
    let context = context_lowercase(text, start, end);
    // An explicit phone cue wins over generic document/reference words such
    // as "facture"; otherwise a real phone in an invoice can be kept readable.
    if has_phone_context(text, start, end) {
        return false;
    }
    const REFERENCE_CUES: &[&str] = &[
        "ref",
        "reference",
        "référence",
        "dossier",
        "n°",
        "no.",
        "annonce",
        "bodacc",
        "facture",
        "devis",
        "publié",
        "période",
    ];
    if REFERENCE_CUES.iter().any(|cue| contains_word_cue(&context, cue)) {
        return true;
    }
    static DATE_CUE: OnceLock<regex::Regex> = OnceLock::new();
    DATE_CUE
        .get_or_init(|| regex::Regex::new(r"\b\d{1,2}[./-]\d{1,2}[./-]\d{2,4}\b").expect("date cue regex"))
        .is_match(&context)
}

fn has_phone_context(text: &str, start: usize, end: usize) -> bool {
    ["téléphone", "telephone", "mobile", "portable", "tel"]
        .iter()
        .any(|cue| has_cue_near_span(text, start, end, cue))
}

/// Match a cue as a word instead of matching it inside another word.
fn contains_word_cue(text: &str, cue: &str) -> bool {
    text.match_indices(cue).any(|(start, _)| {
        let before = text[..start].chars().next_back();
        let after = text[start + cue.len()..].chars().next();
        !before.is_some_and(is_word_char) && !after.is_some_and(is_word_char)
    })
}

/// Return whether a character participates in a cue's word boundary.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Keep all-letter BIC-shaped prose unless banking context is nearby.
fn has_bic_context(text: &str, start: usize, end: usize) -> bool {
    let context = context_lowercase(text, start, end);
    ["bic", "bic8", "bic11", "swift", "iban"]
        .iter()
        .any(|cue| contains_word_cue(&context, cue))
}
