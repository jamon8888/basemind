use std::sync::OnceLock;

use crate::config::RedactionCustomPattern;

/// Expand a byte window to valid UTF-8 boundaries before it is sliced.
pub(super) fn aligned_context_bounds(text: &str, start: usize, end: usize) -> (usize, usize) {
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

/// Build exact custom patterns for valid French NIR candidates.
pub(super) fn valid_fr_nir_patterns(text: &str) -> Vec<RedactionCustomPattern> {
    let regex = regex::Regex::new(
        r"\b[12]\p{White_Space}?\d{2}\p{White_Space}?(?:0[1-9]|1[0-2])\p{White_Space}?(?:\d{2}|2[AB])\p{White_Space}?\d{3}\p{White_Space}?\d{3}\p{White_Space}?\d{2}\b",
    )
    .expect("the static French NIR regex must compile");
    regex
        .find_iter(text)
        .filter(|candidate| crate::pii::validate_fr_nir(candidate.as_str()))
        .map(|candidate| RedactionCustomPattern {
            label: "national_id_number".to_string(),
            pattern: format!(r"\b{}\b", regex::escape(candidate.as_str())),
            case_sensitive: true,
        })
        .collect()
}

/// Decide whether a high-recall engine finding is safe to discard in this legal context.
pub(super) fn is_false_positive_pattern(
    original: &str,
    finding: &xberg::types::redaction::RedactionFinding,
    legal_fr_filters: bool,
) -> bool {
    let start = finding.start as usize;
    let end = finding.end as usize;
    let Some(value) = original.get(start..end) else {
        return false;
    };
    match &finding.category {
        xberg::types::redaction::PiiCategory::Phone => {
            // French phones have ten national digits; an eight-digit run with
            // a reference/date shape is not a phone unless phone context says so.
            legal_fr_filters
                && is_legal_reference_finding(original, start, end, value)
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
    let (context_start, context_end) = aligned_context_bounds(text, start, end);
    let Some(context) = text.get(context_start..context_end).map(str::to_lowercase) else {
        return false;
    };
    const REFERENCE_CUES: &[&str] = &[
        "ref",
        "réf",
        "reference",
        "référence",
        "dossier",
        "case",
        "matter",
        "n°",
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
    let has_no_abbreviation = context
        .split_whitespace()
        .any(|word| word.trim_matches(|character: char| !character.is_alphanumeric() && character != '.') == "no.");
    has_reference_word || has_no_abbreviation
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
    let immediately_before = text.get(context_start..start).and_then(|prefix| {
        let word = prefix
            .split(|character: char| !character.is_alphanumeric())
            .rfind(|word| !word.is_empty())?;
        let word_end = prefix.rfind(word)? + word.len();
        let gap = &prefix[word_end..];
        (!gap.chars().any(is_sentence_boundary)).then_some(word)
    });
    let immediately_after = text.get(end..context_end).and_then(|suffix| {
        let word = suffix
            .split(|character: char| !character.is_alphanumeric())
            .find(|word| !word.is_empty())?;
        let word_start = suffix.find(word)?;
        let gap = &suffix[..word_start];
        (!gap.chars().any(is_sentence_boundary)).then_some(word)
    });
    immediately_before.is_some_and(is_phone_cue) || immediately_after.is_some_and(is_phone_cue)
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
    compact_date || year_reference
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
    let (context_start, context_end) = aligned_context_bounds(text, start, end);
    let Some(context) = text.get(context_start..context_end).map(str::to_lowercase) else {
        return false;
    };
    const DATE_WORD_CUES: &[&str] = &["date", "dated"];
    let explicit_word_cue = context
        .split_whitespace()
        .map(|word| word.trim_matches(|c: char| !c.is_alphabetic()))
        .any(|word| DATE_WORD_CUES.contains(&word));
    if explicit_word_cue {
        return true;
    }
    static TEXT_DATE_CUE: OnceLock<regex::Regex> = OnceLock::new();
    TEXT_DATE_CUE
        .get_or_init(|| {
            regex::Regex::new(
                r"\b\d{1,2}\s+(?:janvier|février|mars|avril|mai|juin|juillet|août|septembre|octobre|novembre|décembre|january|february|march|april|may|june|july|august|september|october|november|december)\s+\d{4}\b",
            )
            .expect("text date cue regex")
        })
        .is_match(&context)
}

/// Return whether an all-letter finding has nearby banking terminology.
fn has_bic_context(text: &str, start: usize, end: usize) -> bool {
    let (context_start, context_end) = aligned_context_bounds(text, start, end);
    text.get(context_start..context_end)
        .map(str::to_ascii_lowercase)
        .is_some_and(|context| ["bic", "swift", "iban"].iter().any(|cue| context.contains(cue)))
}

pub(super) fn rebuild_redacted_text(original: &str, findings: &[xberg::types::redaction::RedactionFinding]) -> String {
    let mut ordered = findings.to_vec();
    ordered.sort_by_key(|finding| (finding.start, finding.end));
    let mut rebuilt = String::with_capacity(original.len());
    let mut cursor = 0usize;
    for finding in ordered {
        let start = finding.start as usize;
        let end = finding.end as usize;
        if start < cursor
            || end > original.len()
            || !original.is_char_boundary(start)
            || !original.is_char_boundary(end)
        {
            continue;
        }
        rebuilt.push_str(&original[cursor..start]);
        rebuilt.push_str(&finding.replacement_token);
        cursor = end;
    }
    rebuilt.push_str(&original[cursor..]);
    rebuilt
}

#[cfg(test)]
mod tests {
    use super::{
        has_explicit_date_cue, has_phone_context, has_reference_cue, is_false_positive_pattern,
        is_legal_reference_finding, rebuild_redacted_text,
    };
    use xberg::types::redaction::{PiiCategory, RedactionFinding, RedactionStrategy};

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
            assert!(!is_false_positive_pattern(ambiguous, &finding, legal_fr_filters));
        }
        let dated = "Jugement du 12 février 2026, annonce 20260219";
        let dated_start = dated.find("20260219").unwrap();
        assert!(has_explicit_date_cue(dated, dated_start, dated_start + 8));
        assert!(is_legal_reference_finding(
            dated,
            dated_start,
            dated_start + 8,
            "20260219"
        ));
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
        assert!(is_false_positive_pattern(year_reference, &year_finding, true));
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
            true
        ));
        assert_eq!(
            rebuild_redacted_text(unrelated_date, &[unrelated_date_finding]),
            "Jugement du 12 février 2026, dossier [PHONE_1]"
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
        assert!(!is_false_positive_pattern(phone_context, &phone_finding, true));

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
        let english_no = "No objection, case 2024-1234";
        let english_no_start = english_no.find("2024-1234").unwrap();
        assert!(has_reference_cue(english_no, english_no_start, english_no_start + 9));
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
            true
        ));
        let abbreviated_no = "No. 2024-1234";
        let abbreviated_no_start = abbreviated_no.find("2024-1234").unwrap();
        assert!(has_reference_cue(
            abbreviated_no,
            abbreviated_no_start,
            abbreviated_no_start + 9
        ));
        let unrelated_short_word = "Date 20260219, the cell in the table, 61234567";
        let unrelated_start = unrelated_short_word.find("61234567").unwrap();
        assert!(!has_phone_context(
            unrelated_short_word,
            unrelated_start,
            unrelated_start + 8
        ));
    }
}
