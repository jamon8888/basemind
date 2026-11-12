use std::collections::HashSet;
use std::sync::OnceLock;

use crate::config::RedactionCustomPattern;

/// Context radius for cue matching. Keep this bounded so a cue in a nearby
/// sentence cannot suppress redaction of an unrelated finding.
const CONTEXT_RADIUS_CHARS: usize = 80;
const MAX_VALID_FR_NIR_PATTERNS: usize = 256;
const DATE_CUE_RADIUS_CHARS: usize = 32;
const REFERENCE_CUE_RADIUS_CHARS: usize = 32;
const DATE_CUE_GAP_BYTES: usize = 16;

/// Return a bounded character window around byte offsets, aligned to UTF-8 boundaries.
pub(super) fn aligned_context_bounds(text: &str, start: usize, end: usize) -> (usize, usize) {
    context_bounds_with_radius(text, start, end, CONTEXT_RADIUS_CHARS)
}

fn context_bounds_with_radius(text: &str, start: usize, end: usize, radius_chars: usize) -> (usize, usize) {
    let mut context_start = start.min(text.len());
    while context_start > 0 && !text.is_char_boundary(context_start) {
        context_start -= 1;
    }
    let mut context_end = end.min(text.len());
    while context_end < text.len() && !text.is_char_boundary(context_end) {
        context_end += 1;
    }
    if radius_chars == 0 {
        return (context_start, context_end);
    }
    let context_start = text[..context_start]
        .char_indices()
        .rev()
        .nth(radius_chars - 1)
        .map(|(index, _)| index)
        .unwrap_or(0);
    let context_end = text[context_end..]
        .char_indices()
        .nth(radius_chars - 1)
        .map(|(index, character)| context_end + index + character.len_utf8())
        .unwrap_or(text.len());
    (context_start, context_end)
}

/// Build exact custom patterns for valid French NIR candidates.
pub(super) fn valid_fr_nir_patterns(text: &str) -> (Vec<RedactionCustomPattern>, bool) {
    static NIR_REGEX: OnceLock<regex::Regex> = OnceLock::new();
    let regex = NIR_REGEX.get_or_init(|| {
        regex::Regex::new(
            r"\b[12]\p{White_Space}?\d{2}\p{White_Space}?(?:0[1-9]|1[0-2])\p{White_Space}?(?:\d{2}|2[AB])\p{White_Space}?\d{3}\p{White_Space}?\d{3}\p{White_Space}?\d{2}\b",
        )
        .expect("the static French NIR regex must compile")
    });
    let max_candidates_to_scan = MAX_VALID_FR_NIR_PATTERNS * 10;
    let candidates: Vec<_> = regex.find_iter(text).take(max_candidates_to_scan + 1).collect();
    let candidate_scan_truncated = candidates.len() > max_candidates_to_scan;
    let mut patterns: Vec<_> = candidates
        .into_iter()
        .filter(|candidate| crate::pii::validate_fr_nir(candidate.as_str()))
        // Bound regex compilation and matching work for untrusted documents. Take one extra
        // candidate so truncation is observable instead of silently dropping PII.
        .take(MAX_VALID_FR_NIR_PATTERNS + 1)
        .map(|candidate| RedactionCustomPattern {
            label: "national_id_number".to_string(),
            pattern: format!(r"\b{}\b", regex::escape(candidate.as_str())),
            case_sensitive: true,
        })
        .collect();
    let truncated = candidate_scan_truncated || patterns.len() > MAX_VALID_FR_NIR_PATTERNS;
    if truncated {
        tracing::warn!(
            max_patterns = MAX_VALID_FR_NIR_PATTERNS,
            max_candidates = max_candidates_to_scan,
            "French NIR redaction pass cap reached; additional identifiers may require a follow-up pass"
        );
        patterns.truncate(MAX_VALID_FR_NIR_PATTERNS);
    }
    if patterns.len() > 1 {
        let alternatives = patterns
            .into_iter()
            .map(|pattern| {
                pattern
                    .pattern
                    .strip_prefix(r"\b")
                    .and_then(|pattern| pattern.strip_suffix(r"\b"))
                    .unwrap_or(&pattern.pattern)
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("|");
        patterns = vec![RedactionCustomPattern {
            label: "national_id_number".to_string(),
            pattern: format!(r"\b(?:{alternatives})\b"),
            case_sensitive: true,
        }];
    }
    (patterns, truncated)
}

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
            super::redact_legal_fr::is_act_date_context(original, start, end)
        }
        xberg::types::redaction::PiiCategory::Phone => {
            // French phones have ten national digits; an eight-digit run with
            // a reference/date shape is not a phone unless phone context says so.
            // A span of years ("exercice 2024-2025") needs no cue: it carries the
            // period a legal reasoning depends on. Eight-digit numbers written 4-4
            // exist abroad, so a dialing prefix or a trailing digit group keeps it masked.
            legal_fr_filters
                && (is_year_span_finding(original, start, end, value)
                    || is_legal_reference_finding(original, start, end, value))
                && !has_phone_context(original, start, end)
        }
        xberg::types::redaction::PiiCategory::SwiftBic => {
            // All-letter BIC-shaped words need a nearby banking cue. This
            // avoids masking ordinary uppercase prose such as ATTESTATION.
            value.chars().all(|c| c.is_ascii_alphabetic()) && !has_bic_context(original, start, end)
        }
        _ => false,
    }
}

/// Return whether the local text names the finding as a legal reference.
fn has_reference_cue(text: &str, start: usize, end: usize) -> bool {
    let (context_start, context_end) = context_bounds_with_radius(text, start, end, REFERENCE_CUE_RADIUS_CHARS);
    let Some(context) = text.get(context_start..context_end).map(str::to_lowercase) else {
        return false;
    };
    const REFERENCE_CUES: &[&str] = &[
        "ref",
        "réf",
        "reference",
        "référence",
        "dossier",
        "annonce",
        "bodacc",
        "facture",
        "devis",
        "publié",
        "période",
    ];
    let has_reference_word = context
        .split(|character: char| !character.is_alphanumeric() && character != '°')
        .filter(|word| !word.is_empty())
        .any(|word| REFERENCE_CUES.contains(&word));
    has_reference_word || has_adjacent_no_cue(text, start, end) || has_adjacent_number_cue(text, start, end)
}

/// Treat `No.` as a reference cue only when it labels the finding itself.
fn has_adjacent_no_cue(text: &str, start: usize, end: usize) -> bool {
    let before = text
        .get(..start)
        .map(|prefix| {
            prefix.trim_end_matches(|character: char| character.is_whitespace() || ":-–—".contains(character))
        })
        .is_some_and(|prefix| has_standalone_suffix(prefix, "no."));
    let after = text
        .get(end..)
        .map(|suffix| {
            suffix.trim_start_matches(|character: char| character.is_whitespace() || ":-–—".contains(character))
        })
        .is_some_and(|suffix| has_standalone_prefix(suffix, "no."));
    before || after
}

/// Treat `n°` as a reference cue only when it labels the finding itself. It is
/// too common in legal prose to use an unrelated occurrence in the wider window.
fn has_adjacent_number_cue(text: &str, start: usize, end: usize) -> bool {
    let before = text
        .get(..start)
        .map(|prefix| {
            prefix.trim_end_matches(|character: char| character.is_whitespace() || ":-–—".contains(character))
        })
        .is_some_and(|prefix| has_standalone_suffix(prefix, "n°"));
    let after = text
        .get(end..)
        .map(|suffix| {
            suffix.trim_start_matches(|character: char| character.is_whitespace() || ":-–—".contains(character))
        })
        .is_some_and(|suffix| has_standalone_prefix(suffix, "n°"));
    before || after
}

fn has_standalone_suffix(value: &str, suffix: &str) -> bool {
    let lower = value.to_lowercase();
    lower.strip_suffix(suffix).is_some_and(|prefix| {
        prefix
            .chars()
            .next_back()
            .is_none_or(|character| !character.is_alphanumeric())
    })
}

fn has_standalone_prefix(value: &str, prefix: &str) -> bool {
    let lower = value.to_lowercase();
    lower.strip_prefix(prefix).is_some_and(|suffix| {
        suffix
            .chars()
            .next()
            .is_none_or(|character| !character.is_alphanumeric())
    })
}

/// Return whether nearby wording explicitly identifies a phone number.
fn has_phone_context(text: &str, start: usize, end: usize) -> bool {
    let (context_start, context_end) = aligned_context_bounds(text, start, end);
    const PHONE_CUES: &[&str] = &[
        "phone",
        "telephone",
        "téléphone",
        "mobile",
        "contact",
        "whatsapp",
        "fax",
    ];
    let is_phone_cue = |word: &str| {
        let normalized = word.to_lowercase();
        PHONE_CUES.contains(&normalized.as_str()) || matches!(normalized.as_str(), "tel" | "tél" | "cell")
    };
    let immediately_before = text
        .get(context_start..start)
        .and_then(nearest_word_before)
        .and_then(|(word, gap)| (!gap.chars().any(is_sentence_boundary)).then_some(word));
    let immediately_after = text
        .get(end..context_end)
        .and_then(nearest_word_after)
        .and_then(|(word, gap)| (!gap.chars().any(is_sentence_boundary)).then_some(word));
    immediately_before.is_some_and(is_phone_cue) || immediately_after.is_some_and(is_phone_cue)
}

/// Return the last alphanumeric word and its trailing separator without rescanning
/// the prefix for a possibly repeated word.
fn nearest_word_before(text: &str) -> Option<(&str, &str)> {
    let mut word_end = text.len();
    for (index, character) in text.char_indices().rev() {
        if character.is_alphanumeric() {
            break;
        }
        word_end = index;
    }
    if word_end == 0 {
        return None;
    }
    let mut word_start = word_end;
    for (index, character) in text[..word_end].char_indices().rev() {
        if !character.is_alphanumeric() {
            break;
        }
        word_start = index;
    }
    Some((&text[word_start..word_end], &text[word_end..]))
}

/// Return the first alphanumeric word and its leading separator without rescanning
/// the suffix for a possibly repeated word.
fn nearest_word_after(text: &str) -> Option<(&str, &str)> {
    let mut word_start = 0;
    for (index, character) in text.char_indices() {
        if character.is_alphanumeric() {
            break;
        }
        word_start = index + character.len_utf8();
    }
    if word_start == text.len() {
        return None;
    }
    let mut word_end = word_start;
    for (index, character) in text[word_start..].char_indices() {
        if !character.is_alphanumeric() {
            break;
        }
        word_end = word_start + index + character.len_utf8();
    }
    Some((&text[word_start..word_end], &text[..word_start]))
}

fn is_sentence_boundary(character: char) -> bool {
    matches!(character, '.' | '!' | '?' | ';' | '\n' | '\r')
}

/// Recognize compact dates and year-prefixed legal reference numbers.
fn is_legal_reference_shape(value: &str) -> bool {
    // Compact dates are exactly YYYYMMDD. Dashed references intentionally
    // require a four-digit year plus a three- or four-digit numeric suffix;
    // shorter or non-numeric forms remain conservative phone candidates.
    let compact_date = value.len() == 8 && value.chars().all(|c| c.is_ascii_digit());
    let year_reference = value.split_once('-').is_some_and(|(year, suffix)| {
        year.len() == 4
            && matches!(year.as_bytes().first(), Some(b'1' | b'2'))
            && year.chars().all(|c| c.is_ascii_digit())
            && (3..=4).contains(&suffix.len())
            && suffix.chars().all(|c| c.is_ascii_digit())
    });
    // Dashed references keep the broader 1000..=2999 legal-document range, while compact dates
    // are restricted to 2000..=2099 below to avoid treating arbitrary eight-digit phone-like
    // values as dates.
    compact_date || year_reference
}

/// A year-shaped value that is not glued to other digits of a longer number.
fn is_year_span_finding(text: &str, start: usize, end: usize, value: &str) -> bool {
    is_year_range(value) && !touches_dialing_digits(text, start, end)
}

/// Whether a dialing prefix (`+55 21`, `(21)`) precedes the value or a digit group follows it,
/// across blanks and dashes.
fn touches_dialing_digits(text: &str, start: usize, end: usize) -> bool {
    let gap = |c: char| c.is_whitespace() || c == '-';
    let before = text[..start].trim_end_matches(gap).chars().next_back();
    let after = text[end..].trim_start_matches(gap).chars().next();
    matches!(before, Some(c) if c.is_ascii_digit() || c == '+' || c == ')')
        || matches!(after, Some(c) if c.is_ascii_digit())
}

/// Recognize a span of years such as `2024-2025` or `2019-2024`: two years between
/// 1900 and 2099, the second one to ten years after the first.
fn is_year_range(value: &str) -> bool {
    let Some((first, second)) = value.split_once('-') else {
        return false;
    };
    let year = |part: &str| {
        (part.len() == 4 && part.chars().all(|c| c.is_ascii_digit()))
            .then(|| part.parse::<u32>().ok())
            .flatten()
            .filter(|year| (1900..=2099).contains(year))
    };
    match (year(first), year(second)) {
        (Some(first), Some(second)) => (1..=10).contains(&second.saturating_sub(first)),
        _ => false,
    }
}

/// Decide whether a phone-shaped finding is a legal reference in context.
fn is_legal_reference_finding(text: &str, start: usize, end: usize, value: &str) -> bool {
    if !is_legal_reference_shape(value) {
        return false;
    }
    if value.len() == 8 && value.chars().all(|character| character.is_ascii_digit()) {
        return is_valid_compact_date(value) && has_explicit_date_cue(text, start, end);
    }
    has_reference_cue(text, start, end)
}

/// Validate a compact `YYYYMMDD` value using the Gregorian month lengths.
fn is_valid_compact_date(value: &str) -> bool {
    if value.len() != 8 || !value.chars().all(|character| character.is_ascii_digit()) {
        return false;
    }
    let Ok(year) = value[0..4].parse::<u32>() else {
        return false;
    };
    // The legal profile targets contemporary client files; older compact dates
    // remain phone candidates instead of being silently treated as references.
    if !(2000..=2099).contains(&year) {
        return false;
    }
    let Ok(month) = value[4..6].parse::<u32>() else {
        return false;
    };
    let Ok(day) = value[6..8].parse::<u32>() else {
        return false;
    };
    let days_in_month = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 400 == 0 || (year % 4 == 0 && year % 100 != 0) => 29,
        2 => 28,
        _ => return false,
    };
    (1..=days_in_month).contains(&day)
}

/// Return whether a compact date has an explicit nearby date cue.
fn has_explicit_date_cue(text: &str, start: usize, end: usize) -> bool {
    let (context_start, context_end) = context_bounds_with_radius(text, start, end, DATE_CUE_RADIUS_CHARS);
    let Some(context) = text.get(context_start..context_end).map(str::to_lowercase) else {
        return false;
    };
    const DATE_WORD_CUES: &[&str] = &["date", "dated"];
    let explicit_word_cue = [
        text.get(context_start..start).and_then(nearest_word_before),
        text.get(end..context_end).and_then(nearest_word_after),
    ]
    .into_iter()
    .flatten()
    .any(|(word, gap)| {
        gap.len() <= DATE_CUE_GAP_BYTES
            && !gap.chars().any(is_sentence_boundary)
            && DATE_WORD_CUES.contains(&word.to_ascii_lowercase().as_str())
    });
    if explicit_word_cue {
        return true;
    }
    static TEXT_DATE_CUE: OnceLock<regex::Regex> = OnceLock::new();
    const MAX_DATE_CUE_MATCHES: usize = 8;
    TEXT_DATE_CUE
        .get_or_init(|| {
            regex::Regex::new(
                r"\b\d{1,2}\s+(?:janvier|février|mars|avril|mai|juin|juillet|août|septembre|octobre|novembre|décembre|january|february|march|april|may|june|july|august|september|october|november|december)\s+\d{4}\b",
            )
            .expect("text date cue regex")
        })
        .find_iter(&context)
        .take(MAX_DATE_CUE_MATCHES)
        .any(|date| {
            let (Some(relative_start), Some(relative_end)) =
                (start.checked_sub(context_start), end.checked_sub(context_start))
            else {
                return false;
            };
            if relative_start > context.len() || relative_end > context.len() {
                return false;
            }
            (date.end() <= relative_start && relative_start - date.end() <= DATE_CUE_GAP_BYTES)
                || (date.start() >= relative_end && date.start() - relative_end <= DATE_CUE_GAP_BYTES)
        })
}

/// Return whether an all-letter finding has nearby banking terminology.
fn has_bic_context(text: &str, start: usize, end: usize) -> bool {
    let (context_start, context_end) = aligned_context_bounds(text, start, end);
    text.get(context_start..context_end)
        .map(str::to_ascii_lowercase)
        .is_some_and(|context| {
            context
                .split(|character: char| !character.is_alphanumeric())
                .any(|word| matches!(word, "bic" | "swift" | "iban"))
        })
}

pub(super) fn rebuild_redacted_text(original: &str, findings: &[xberg::types::redaction::RedactionFinding]) -> String {
    let mut ordered = findings.to_vec();
    ordered.sort_by_key(|finding| (finding.start, finding.end));
    let mut rebuilt = String::with_capacity(original.len());
    let mut cursor = 0usize;
    for finding in ordered {
        let start = finding.start as usize;
        let end = finding.end as usize;
        if start > end
            || start > original.len()
            || end > original.len()
            || !original.is_char_boundary(start)
            || !original.is_char_boundary(end)
        {
            continue;
        }
        if start < cursor {
            // Do not emit `original[cursor..start]`: it overlaps the already
            // redacted span and could reintroduce PII. A second token is safer
            // than preserving source text, even when the overlap changes the
            // output length. The second token covers its full finding, including
            // the part after `cursor`, so moving the cursor to `end` is intentional.
            if end > cursor {
                rebuilt.push_str(&finding.replacement_token);
                cursor = end;
            }
            continue;
        }
        rebuilt.push_str(&original[cursor..start]);
        rebuilt.push_str(&finding.replacement_token);
        cursor = end;
    }
    rebuilt.push_str(&original[cursor..]);
    rebuilt
}

/// Return replacement tokens for findings that the overlap policy skips.
pub(super) fn overlapping_replacement_tokens(
    original: &str,
    findings: &[xberg::types::redaction::RedactionFinding],
) -> HashSet<String> {
    let mut ordered = findings.to_vec();
    ordered.sort_by_key(|finding| (finding.start, finding.end));
    let mut skipped = HashSet::new();
    let mut represented = HashSet::new();
    let mut cursor = 0usize;
    for finding in ordered {
        let start = finding.start as usize;
        let end = finding.end as usize;
        if start > end
            || start > original.len()
            || end > original.len()
            || !original.is_char_boundary(start)
            || !original.is_char_boundary(end)
        {
            continue;
        }
        if start < cursor {
            if end > cursor {
                represented.insert(finding.replacement_token);
                cursor = end;
            } else {
                skipped.insert(finding.replacement_token);
            }
            continue;
        }
        represented.insert(finding.replacement_token);
        cursor = end;
    }
    skipped.retain(|token| !represented.contains(token));
    skipped
}

#[cfg(test)]
mod tests {
    use super::{
        has_bic_context, has_explicit_date_cue, has_phone_context, has_reference_cue, is_false_positive_pattern,
        is_legal_reference_finding, is_valid_compact_date, is_year_range, overlapping_replacement_tokens,
        rebuild_redacted_text, valid_fr_nir_patterns,
    };
    use xberg::types::redaction::{PiiCategory, RedactionFinding, RedactionStrategy};

    fn phone_at(text: &str, value: &str) -> RedactionFinding {
        let start = text.find(value).unwrap();
        RedactionFinding {
            start: start as u32,
            end: (start + value.len()) as u32,
            category: PiiCategory::Phone,
            strategy: RedactionStrategy::Mask,
            replacement_token: "[PHONE_1]".to_string(),
        }
    }

    #[test]
    fn a_span_of_years_stays_readable_without_a_reference_cue() {
        for (text, value) in [
            (
                "Encours moyen mensuel sur 2024-2025 d'environ 25 000 € HT.",
                "2024-2025",
            ),
            ("Chiffre d'affaires 2019-2024 en baisse.", "2019-2024"),
        ] {
            assert!(is_year_range(value), "{value}");
            for legal_fr_v2 in [false, true] {
                assert!(
                    is_false_positive_pattern(text, &phone_at(text, value), true, legal_fr_v2, false),
                    "{text}"
                );
            }
            assert!(
                !is_false_positive_pattern(text, &phone_at(text, value), false, false, false),
                "{text}"
            );
        }
        // A phone cue glued to the value still wins.
        let phone = "Tél : 2024-2025, du lundi au vendredi";
        for legal_fr_v2 in [false, true] {
            assert!(!is_false_positive_pattern(
                phone,
                &phone_at(phone, "2024-2025"),
                true,
                legal_fr_v2,
                false
            ));
        }
        // Some countries write eight-digit numbers as two groups of four: a dialing prefix or a
        // trailing digit group makes the span part of a longer number, which stays redacted.
        for text in [
            "Joignable au (21) 2012-2019 en semaine.",
            "Joignable au +55 21 2012-2019 en semaine.",
            "Joignable au +2012-2019 en semaine.",
            "Joignable au 2012-2019 44 en semaine.",
        ] {
            for legal_fr_v2 in [false, true] {
                assert!(
                    !is_false_positive_pattern(text, &phone_at(text, "2012-2019"), true, legal_fr_v2, false),
                    "{text}"
                );
            }
        }
        // Brackets and punctuation around a real span keep it readable.
        for text in [
            "Les exercices (2019-2021) sont clos.",
            "Exercices 2019-2021 : 3 M€ de pertes.",
        ] {
            for legal_fr_v2 in [false, true] {
                assert!(
                    is_false_positive_pattern(text, &phone_at(text, "2019-2021"), true, legal_fr_v2, false),
                    "{text}"
                );
            }
        }
        for value in [
            "2025-2024",
            "2024-2024",
            "2020-2031",
            "1899-1900",
            "2099-2100",
            "2024-25",
            "2024-202",
        ] {
            assert!(!is_year_range(value), "{value}");
        }
    }

    #[test]
    fn ambiguous_eight_digit_phone_is_not_treated_as_a_reference() {
        let ambiguous = "Dossier 61234567";
        let ambiguous_start = ambiguous.find("61234567").unwrap();
        for legal_fr_filters in [false, true] {
            let finding = RedactionFinding {
                start: ambiguous_start as u32,
                end: (ambiguous_start + 8) as u32,
                category: PiiCategory::Phone,
                strategy: RedactionStrategy::Mask,
                replacement_token: "[PHONE_1]".to_string(),
            };
            assert!(!is_false_positive_pattern(
                ambiguous,
                &finding,
                legal_fr_filters,
                false,
                false
            ));
        }
        let dated = "Jugement du 12 février 2026, annonce 20260219";
        let dated_start = dated.find("20260219").unwrap();
        assert!(has_explicit_date_cue(dated, dated_start, dated_start + 8));
        let unrelated_date_word = "Date de naissance, 20260219";
        let unrelated_date_word_start = unrelated_date_word.find("20260219").unwrap();
        assert!(!has_explicit_date_cue(
            unrelated_date_word,
            unrelated_date_word_start,
            unrelated_date_word_start + 8
        ));
        assert!(is_legal_reference_finding(
            dated,
            dated_start,
            dated_start + 8,
            "20260219"
        ));
        assert!(is_valid_compact_date("20250131"));
        assert!(!is_valid_compact_date("19991231"));
        assert!(!is_valid_compact_date("21000101"));
        let many_nirs = "185071510000058 ".repeat(300);
        let (patterns, truncated) = valid_fr_nir_patterns(&many_nirs);
        assert_eq!(patterns.len(), 1);
        assert!(truncated);
        let year_reference = "Référence 2024-2025";
        let year_reference_start = year_reference.find("2024-2025").unwrap();
        let year_finding = RedactionFinding {
            start: year_reference_start as u32,
            end: (year_reference_start + 9) as u32,
            category: PiiCategory::Phone,
            strategy: RedactionStrategy::Mask,
            replacement_token: "[PHONE_1]".to_string(),
        };
        assert!(is_legal_reference_finding(
            year_reference,
            year_reference_start,
            year_reference_start + 9,
            "2024-2025"
        ));
        assert!(is_false_positive_pattern(
            year_reference,
            &year_finding,
            true,
            false,
            false
        ));
        let substring_cue = "prefix 2024-2025";
        let substring_start = substring_cue.find("2024-2025").unwrap();
        assert!(!is_legal_reference_finding(
            substring_cue,
            substring_start,
            substring_start + 9,
            "2024-2025"
        ));
        let unrelated_numeric_date = "12/03/2026, 2024-2025";
        let unrelated_start = unrelated_numeric_date.find("2024-2025").unwrap();
        assert!(!is_legal_reference_finding(
            unrelated_numeric_date,
            unrelated_start,
            unrelated_start + 9,
            "2024-2025"
        ));
        let page_number = "Page No. 5, 2024-1234";
        let page_reference_start = page_number.find("2024-1234").unwrap();
        assert!(!has_reference_cue(
            page_number,
            page_reference_start,
            page_reference_start + 9
        ));
        let labeled_reference = "Page No. 2024-1234";
        let labeled_reference_start = labeled_reference.find("2024-1234").unwrap();
        assert!(has_reference_cue(
            labeled_reference,
            labeled_reference_start,
            labeled_reference_start + 9
        ));
        let unrelated_date = "Jugement du 12 février 2026, dossier 61234567";
        let unrelated_date_start = unrelated_date.find("61234567").unwrap();
        let unrelated_date_finding = RedactionFinding {
            start: unrelated_date_start as u32,
            end: (unrelated_date_start + 8) as u32,
            category: PiiCategory::Phone,
            strategy: RedactionStrategy::Mask,
            replacement_token: "[PHONE_1]".to_string(),
        };
        assert!(!is_legal_reference_finding(
            unrelated_date,
            unrelated_date_start,
            unrelated_date_start + 8,
            "61234567"
        ));
        assert!(!is_false_positive_pattern(
            unrelated_date,
            &unrelated_date_finding,
            true,
            false,
            false
        ));
        assert_eq!(
            rebuild_redacted_text(unrelated_date, &[unrelated_date_finding]),
            "Jugement du 12 février 2026, dossier [PHONE_1]"
        );
        let overlapping = [
            RedactionFinding {
                start: 1,
                end: 4,
                category: PiiCategory::Phone,
                strategy: RedactionStrategy::Mask,
                replacement_token: "[A]".to_string(),
            },
            RedactionFinding {
                start: 3,
                end: 6,
                category: PiiCategory::Phone,
                strategy: RedactionStrategy::Mask,
                replacement_token: "[B]".to_string(),
            },
        ];
        assert_eq!(rebuild_redacted_text("abcdef", &overlapping), "a[A][B]");
        let partially_overlapping = [
            RedactionFinding {
                start: 2,
                end: 8,
                category: PiiCategory::Phone,
                strategy: RedactionStrategy::Mask,
                replacement_token: "[A]".to_string(),
            },
            RedactionFinding {
                start: 6,
                end: 10,
                category: PiiCategory::Phone,
                strategy: RedactionStrategy::Mask,
                replacement_token: "[B]".to_string(),
            },
        ];
        assert_eq!(
            rebuild_redacted_text("abcdefghijkl", &partially_overlapping),
            "ab[A][B]kl"
        );
        assert_eq!(
            overlapping_replacement_tokens("abcdef", &overlapping),
            std::collections::HashSet::new()
        );
        let shared_token = [
            RedactionFinding {
                start: 1,
                end: 4,
                category: PiiCategory::Phone,
                strategy: RedactionStrategy::Mask,
                replacement_token: "[PHONE_1]".to_string(),
            },
            RedactionFinding {
                start: 3,
                end: 4,
                category: PiiCategory::Phone,
                strategy: RedactionStrategy::Mask,
                replacement_token: "[PHONE_1]".to_string(),
            },
            RedactionFinding {
                start: 7,
                end: 9,
                category: PiiCategory::Phone,
                strategy: RedactionStrategy::Mask,
                replacement_token: "[PHONE_1]".to_string(),
            },
        ];
        assert_eq!(
            overlapping_replacement_tokens("abcdefghi", &shared_token),
            std::collections::HashSet::new()
        );
        let month_only = "En janvier, dossier 61234567";
        let month_only_start = month_only.find("61234567").unwrap();
        assert!(!has_explicit_date_cue(
            month_only,
            month_only_start,
            month_only_start + 8
        ));
        assert!(!is_legal_reference_finding(
            month_only,
            month_only_start,
            month_only_start + 8,
            "61234567"
        ));

        let phone_context = "Date 20260219, téléphone 61234567";
        let phone_start = phone_context.find("61234567").unwrap();
        let phone_finding = RedactionFinding {
            start: phone_start as u32,
            end: (phone_start + 8) as u32,
            category: PiiCategory::Phone,
            strategy: RedactionStrategy::Mask,
            replacement_token: "[PHONE_1]".to_string(),
        };
        assert!(has_phone_context(phone_context, phone_start, phone_start + 8));
        assert!(!is_false_positive_pattern(
            phone_context,
            &phone_finding,
            true,
            false,
            false
        ));

        let substring_context = "Date 20260219, cellule 61234567";
        let substring_start = substring_context.find("61234567").unwrap();
        assert!(!has_phone_context(
            substring_context,
            substring_start,
            substring_start + 8
        ));

        let adjacent_abbreviation = "Date 20260219, tel: 61234567";
        let adjacent_start = adjacent_abbreviation.find("61234567").unwrap();
        assert!(has_phone_context(
            adjacent_abbreviation,
            adjacent_start,
            adjacent_start + 8
        ));
        let uppercase_abbreviation = "Date 20260219, TEL: 61234567";
        let uppercase_start = uppercase_abbreviation.find("61234567").unwrap();
        assert!(has_phone_context(
            uppercase_abbreviation,
            uppercase_start,
            uppercase_start + 8
        ));
        let accented_abbreviation = "Date 20260219, tél: 61234567";
        let accented_start = accented_abbreviation.find("61234567").unwrap();
        assert!(has_phone_context(
            accented_abbreviation,
            accented_start,
            accented_start + 8
        ));
        let embedded_abbreviation = "Date 20260219, tele61234567";
        let embedded_start = embedded_abbreviation.find("61234567").unwrap();
        assert!(!has_phone_context(
            embedded_abbreviation,
            embedded_start,
            embedded_start + 8
        ));
        let unrelated_telephone_word = "Date 20260219, television 61234567";
        let unrelated_telephone_start = unrelated_telephone_word.find("61234567").unwrap();
        assert!(!has_phone_context(
            unrelated_telephone_word,
            unrelated_telephone_start,
            unrelated_telephone_start + 8
        ));
        let trailing_phone_context = "Date 20260219, 61234567 mobile";
        let trailing_start = trailing_phone_context.find("61234567").unwrap();
        assert!(has_phone_context(
            trailing_phone_context,
            trailing_start,
            trailing_start + 8
        ));
        let distant_abbreviation = format!("Date 20260219, cell{}61234567", " ".repeat(100));
        let distant_start = distant_abbreviation.find("61234567").unwrap();
        assert!(!has_phone_context(
            &distant_abbreviation,
            distant_start,
            distant_start + 8
        ));
        let ordinary_call = "The call was recorded in the minutes, 61234567";
        let ordinary_call_start = ordinary_call.find("61234567").unwrap();
        assert!(!has_phone_context(
            ordinary_call,
            ordinary_call_start,
            ordinary_call_start + 8
        ));
        let distant_phone_context = "Période 2024-2025. Contact : 06 12 34 56 78";
        let distant_phone_start = distant_phone_context.find("2024-2025").unwrap();
        assert!(!has_phone_context(
            distant_phone_context,
            distant_phone_start,
            distant_phone_start + 9
        ));
        let english_reference = "No objection, reference 2024-1234";
        let english_reference_start = english_reference.find("2024-1234").unwrap();
        assert!(has_reference_cue(
            english_reference,
            english_reference_start,
            english_reference_start + 9
        ));
        let common_case = "No objection, in this case 2024-1234";
        let common_case_start = common_case.find("2024-1234").unwrap();
        assert!(!has_reference_cue(
            common_case,
            common_case_start,
            common_case_start + 9
        ));
        let english_no_without_case = "No objection raised, 2024-1234";
        let english_no_without_case_start = english_no_without_case.find("2024-1234").unwrap();
        assert!(!has_reference_cue(
            english_no_without_case,
            english_no_without_case_start,
            english_no_without_case_start + 9
        ));
        let three_digit_reference = "Réf. 2024-123";
        let three_digit_start = three_digit_reference.find("2024-123").unwrap();
        let three_digit_finding = RedactionFinding {
            start: three_digit_start as u32,
            end: (three_digit_start + 8) as u32,
            category: PiiCategory::Phone,
            strategy: RedactionStrategy::Mask,
            replacement_token: "[PHONE_1]".to_string(),
        };
        assert!(is_false_positive_pattern(
            three_digit_reference,
            &three_digit_finding,
            true,
            false,
            false
        ));
        let abbreviated_no = "No. 2024-1234";
        let abbreviated_no_start = abbreviated_no.find("2024-1234").unwrap();
        assert!(has_reference_cue(
            abbreviated_no,
            abbreviated_no_start,
            abbreviated_no_start + 9
        ));
        let embedded_no = "Anno. 2024-1234";
        let embedded_no_start = embedded_no.find("2024-1234").unwrap();
        assert!(!has_reference_cue(
            embedded_no,
            embedded_no_start,
            embedded_no_start + 9
        ));
        let adjacent_number_cue = "n° 61234567";
        let adjacent_number_start = adjacent_number_cue.find("61234567").unwrap();
        assert!(has_reference_cue(
            adjacent_number_cue,
            adjacent_number_start,
            adjacent_number_start + 8
        ));
        let embedded_number_cue = "conn° 61234567";
        let embedded_number_start = embedded_number_cue.find("61234567").unwrap();
        assert!(!has_reference_cue(
            embedded_number_cue,
            embedded_number_start,
            embedded_number_start + 8
        ));
        let unrelated_number_cue = "Pièce n° 3, 61234567";
        let unrelated_number_start = unrelated_number_cue.find("61234567").unwrap();
        assert!(!has_reference_cue(
            unrelated_number_cue,
            unrelated_number_start,
            unrelated_number_start + 8
        ));
        let distant_reference = format!("Réf. {}2024-1234", "texte ".repeat(8));
        let distant_reference_start = distant_reference.find("2024-1234").unwrap();
        assert!(!has_reference_cue(
            &distant_reference,
            distant_reference_start,
            distant_reference_start + 9
        ));
        let unrelated_short_word = "Date 20260219, the cell in the table, 61234567";
        let unrelated_start = unrelated_short_word.find("61234567").unwrap();
        assert!(!has_phone_context(
            unrelated_short_word,
            unrelated_start,
            unrelated_start + 8
        ));

        let bic_context = "BIC ABCDEFGH";
        let bic_start = bic_context.find("ABCDEFGH").unwrap();
        assert!(has_bic_context(bic_context, bic_start, bic_start + 8));
        let unrelated_bic_context = "A bicycle ABCDEFGH";
        let unrelated_bic_start = unrelated_bic_context.find("ABCDEFGH").unwrap();
        assert!(!has_bic_context(
            unrelated_bic_context,
            unrelated_bic_start,
            unrelated_bic_start + 8
        ));
    }
}
