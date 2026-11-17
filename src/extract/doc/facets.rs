//! Deterministic document facet normalization.
//!
//! Facets are deliberately conservative: values outside the curated vocabulary become absent
//! (`None`) instead of becoming an unqueryable singleton or an inferred guess.

const DOC_TYPES: &[&str] = &["pleading", "contract", "memo", "email", "statute", "exhibit", "web"];
const SECTIONS: &[&str] = &[
    "caption",
    "facts",
    "issues",
    "argument",
    "holding",
    "reasoning",
    "signature",
];

/// Normalize a caller-curated document type, rejecting unknown values.
pub(crate) fn doc_type(value: Option<&str>) -> Option<String> {
    normalize_vocab(value, DOC_TYPES)
}

/// Normalize a caller-curated section, rejecting unknown values.
pub(crate) fn section(value: Option<&str>) -> Option<String> {
    normalize_vocab(value, SECTIONS)
}

/// Normalize an ISO-ish jurisdiction code without trying to infer one from prose.
pub(crate) fn jurisdiction(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty()
        || value.len() > 32
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return None;
    }
    Some(value.to_ascii_uppercase())
}

/// Normalize an ISO-8601 calendar date (`YYYY-MM-DD`).
pub(crate) fn doc_date(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    let bytes = value.as_bytes();
    if bytes.len() != 10
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || !bytes[..4]
            .iter()
            .chain(&bytes[5..7])
            .chain(&bytes[8..])
            .all(u8::is_ascii_digit)
    {
        return None;
    }
    let year: u32 = value[..4].parse().ok()?;
    let month: u32 = value[5..7].parse().ok()?;
    let day: u32 = value[8..].parse().ok()?;
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let max_day = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    (1..=max_day).contains(&day).then(|| value.to_string())
}

/// Classify a Markdown heading breadcrumb without guessing when no rule matches.
pub(crate) fn section_from_heading(heading_path: Option<&str>) -> Option<String> {
    let heading = heading_path?.to_ascii_lowercase();
    let rules: &[(&str, &[&str])] = &[
        ("caption", &["caption", "case name", "docket"]),
        ("facts", &["facts", "background"]),
        ("issues", &["issues", "question presented"]),
        ("argument", &["argument", "arguments", "analysis"]),
        ("holding", &["holding", "decision", "disposition"]),
        ("reasoning", &["reasoning", "discussion"]),
        ("signature", &["signature", "signed"]),
    ];
    rules.iter().find_map(|(section, needles)| {
        needles
            .iter()
            .any(|needle| starts_word(&heading, needle))
            .then(|| (*section).to_string())
    })
}

fn normalize_vocab(value: Option<&str>, vocabulary: &[&str]) -> Option<String> {
    let value = value?.trim().to_ascii_lowercase();
    (!value.is_empty() && vocabulary.contains(&value.as_str())).then_some(value)
}

fn starts_word(heading: &str, needle: &str) -> bool {
    let finder = memchr::memmem::Finder::new(needle.as_bytes());
    finder.find_iter(heading.as_bytes()).any(|start| {
        heading[..start]
            .chars()
            .next_back()
            .is_none_or(|character| !character.is_alphanumeric())
    })
}

#[cfg(test)]
mod tests {
    use super::{doc_date, doc_type, jurisdiction, section, section_from_heading};

    #[test]
    fn unknown_values_degrade_to_absent() {
        assert_eq!(doc_type(Some("statut")), None);
        assert_eq!(section(Some("body")), None);
    }

    #[test]
    fn curated_values_are_case_folded() {
        assert_eq!(doc_type(Some(" CONTRACT ")), Some("contract".into()));
        assert_eq!(jurisdiction(Some("fr-ca")), Some("FR-CA".into()));
    }

    #[test]
    fn dates_require_iso_calendar_shape() {
        assert_eq!(doc_date(Some("2026-11-03")), Some("2026-11-03".into()));
        assert_eq!(doc_date(Some("11/03/2026")), None);
        assert_eq!(doc_date(Some("2026-02-31")), None);
        assert_eq!(doc_date(Some("2024-02-29")), Some("2024-02-29".into()));
    }

    #[test]
    fn section_rules_leave_unrecognized_headings_absent() {
        assert_eq!(
            section_from_heading(Some("IV. Holding > Discussion")),
            Some("holding".into())
        );
        assert_eq!(section_from_heading(Some("IV. Termination")), None);
        assert_eq!(section_from_heading(Some("Counterfacts")), None);
    }
}
