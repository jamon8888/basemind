//! French legal redaction policy shared by the model and pattern passes.

use super::tools_redact_filters::aligned_context_bounds;

/// Decide whether a `legal-fr@2` model span is a known false positive.
#[cfg(test)]
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
    let normalized = normalize_geography_value(value).replace('-', " ");
    BROAD_GEOGRAPHY
        .iter()
        .map(|entry| normalize_geography_value(entry).replace('-', " "))
        .any(|entry| entry == normalized)
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
pub(super) fn is_act_date_context(text: &str, start: usize, end: usize) -> bool {
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
    .any(|cue| has_preceding_cue_near_span(text, start, end, cue))
}

/// Match a cue at the candidate occurrence, not merely somewhere in its
/// bounded context window. This prevents one date from borrowing another
/// date's birth/act label in the same paragraph.
fn has_cue_near_span(text: &str, start: usize, end: usize, cue: &str) -> bool {
    has_cue_near_span_in_direction(text, start, end, cue, true)
}

/// Match a cue immediately before a span, without borrowing context from a
/// later candidate in the same sentence.
fn has_preceding_cue_near_span(text: &str, start: usize, end: usize, cue: &str) -> bool {
    has_cue_near_span_in_direction(text, start, end, cue, false)
}

fn has_cue_near_span_in_direction(text: &str, start: usize, end: usize, cue: &str, allow_following_cue: bool) -> bool {
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
            || (allow_following_cue && cue_start >= end && cue_start - end <= MAX_CUE_DISTANCE)
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

/// Return whether a character participates in a cue's word boundary.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}
