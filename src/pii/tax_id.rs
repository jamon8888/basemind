//! Re-labelling a French company identifier from `phone` to `tax_id`.
//!
//! The detection engine files any run of seven to fifteen digits as a phone number, so a
//! SIREN or a SIRET arrives under the `phone` label. For a legal file that is the wrong
//! label in both directions:
//!
//! - a phone number is not a category a legal tool may pass on in the clear;
//! - a company identifier is (it is what BODACC is queried with), and the caller has to be
//!   able to tell the two apart.
//!
//! So a finding whose matched value passes the SIREN/SIRET checksum is re-labelled
//! `tax_id` — the label the caller already knows as the one category it may forward.
//!
//! The re-labelling happens after the engine has run: xberg resolves an overlap in favour of
//! the earlier, longer match, and the built-in phone pattern always comes first, so a
//! custom pattern for the same span would lose the tie. Deciding afterwards is the only
//! place the value is still known.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use xberg::types::redaction::{PiiCategory, RedactionFinding};

use super::siren::{TAX_ID_LABEL, is_fr_company_id};

/// The category a re-labelled finding carries.
pub fn tax_id_category() -> PiiCategory {
    PiiCategory::Custom(TAX_ID_LABEL.to_string())
}

/// True when `category` is the tax-id label.
pub fn is_tax_id(category: &PiiCategory) -> bool {
    matches!(category, PiiCategory::Custom(label) if label == TAX_ID_LABEL)
}

/// Re-label the findings whose matched value is a French company identifier.
///
/// `original` is the text the findings' offsets index (the pre-redaction content). Returns
/// how many findings were re-labelled.
pub fn relabel_in_text(findings: &mut [RedactionFinding], original: &str) -> usize {
    relabel_with(findings, |finding| {
        let start = finding.start as usize;
        let end = finding.end as usize;
        original.get(start..end).map(str::to_string)
    })
}

/// Re-label the findings whose replaced value is a French company identifier.
///
/// `map` is the rehydration map (token → the value the token replaced), which is how the
/// value is recovered when the original text is no longer at hand. Returns how many findings
/// were re-labelled.
pub fn relabel_by_map(findings: &mut [RedactionFinding], map: &HashMap<String, String>) -> usize {
    relabel_with(findings, |finding| map.get(&finding.replacement_token).cloned())
}

/// The shared re-labelling pass: a finding filed as a phone number whose value is a
/// company identifier becomes a tax id.
fn relabel_with(findings: &mut [RedactionFinding], value_of: impl Fn(&RedactionFinding) -> Option<String>) -> usize {
    let mut relabelled = 0;
    for finding in findings.iter_mut() {
        if !matches!(finding.category, PiiCategory::Phone) {
            continue;
        }
        let Some(value) = value_of(finding) else {
            continue;
        };
        if !is_fr_company_id(&value) {
            continue;
        }
        finding.category = tax_id_category();
        relabelled += 1;
    }
    relabelled
}

/// Rename the tokens of the re-labelled findings, in the text and in the map.
///
/// The engine named a company identifier `[PHONE_3]`. The mirror should not: the token
/// carries the label the caller acts on, so a company identifier gets a `[TAX_ID_n]` token
/// of its own. The map is re-keyed with it, so the text and the map stay in step.
///
/// Destination tokens are handed out as `[TAX_ID_1]` upward, skipping any number that is
/// already claimed — by a map key, by a source token, by a token the engine issued under
/// the tax-id label (a custom-pattern finding carries `[TAX_ID_n]` on its own — renaming
/// it would only renumber the fleet — but its number stays taken), or by an occurrence in
/// the redacted text. Claiming makes the rewrite safe as a plain sweep: a destination is
/// never a source (so a later replacement cannot rewrite an earlier destination) and never
/// merges two different values in the map.
///
/// Returns how many tokens were renamed.
pub fn rename_tokens(text: &mut String, map: &mut BTreeMap<String, String>, findings: &[RedactionFinding]) -> usize {
    // The claim set a destination has to stay clear of: every map key, every source token
    // and every token already issued under the tax-id label.
    let mut taken: BTreeSet<String> = map.keys().cloned().collect();

    let mut renames: Vec<(String, String)> = Vec::new();
    let mut next = 0usize; // how many destination numbers have been considered so far
    for finding in findings {
        if !is_tax_id(&finding.category) {
            continue;
        }
        let from = finding.replacement_token.clone();
        if is_tax_id_token(&from) {
            taken.insert(from);
            continue;
        }
        if renames.iter().any(|(existing, _)| *existing == from) {
            continue; // one source, one destination — however many findings share the token
        }
        taken.insert(from.clone());
        let to = loop {
            next += 1;
            let candidate = format!("[{}_{}]", TAX_ID_LABEL.to_ascii_uppercase(), next);
            if !taken.contains(&candidate) && !text.contains(&candidate) {
                break candidate;
            }
        };
        renames.push((from, to));
    }
    for (from, to) in &renames {
        *text = text.replace(from.as_str(), to);
        if let Some(value) = map.remove(from) {
            map.insert(to.clone(), value);
        }
    }
    renames.len()
}

/// True for a token the engine itself issued under the tax-id label (`[TAX_ID_n]`): the
/// sanctioned shape a re-labelled finding must adopt, and a number a new destination must
/// not claim.
fn is_tax_id_token(token: &str) -> bool {
    token.starts_with(&format!("[{}_", TAX_ID_LABEL.to_ascii_uppercase())) && token.ends_with(']')
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A finding over the first occurrence of `value` in `original`.
    fn finding_in(original: &str, value: &str, category: PiiCategory, token: &str) -> RedactionFinding {
        let start = original.find(value).expect("value in text");
        RedactionFinding {
            start: start as u32,
            end: (start + value.len()) as u32,
            category,
            strategy: xberg::types::redaction::RedactionStrategy::TokenReplace,
            replacement_token: token.to_string(),
        }
    }

    #[test]
    fn a_siren_filed_as_a_phone_becomes_a_tax_id() {
        let original = "SIREN 552100554.";
        let mut findings = vec![finding_in(original, "552100554", PiiCategory::Phone, "[PHONE_1]")];
        let relabelled = relabel_in_text(&mut findings, original);
        assert_eq!(relabelled, 1);
        assert!(is_tax_id(&findings[0].category));
    }

    #[test]
    fn a_real_phone_number_stays_a_phone_number() {
        let original = "Appelez le 0612345678.";
        let mut findings = vec![finding_in(original, "0612345678", PiiCategory::Phone, "[PHONE_1]")];
        assert_eq!(relabel_in_text(&mut findings, original), 0);
        assert!(matches!(findings[0].category, PiiCategory::Phone));
    }

    #[test]
    fn the_map_carries_the_value_when_the_text_is_gone() {
        let mut findings = vec![finding(PiiCategory::Phone, "552100554", "[PHONE_1]")];
        let map = HashMap::from([("[PHONE_1]".to_string(), "552 100 554".to_string())]);
        assert_eq!(relabel_by_map(&mut findings, &map), 1);
        assert!(is_tax_id(&findings[0].category));
    }

    #[test]
    fn a_finding_of_another_category_is_never_relabelled() {
        let mut findings = vec![finding(PiiCategory::Email, "552100554", "[EMAIL_1]")];
        assert_eq!(relabel_in_text(&mut findings, "552100554"), 0);
        assert!(matches!(findings[0].category, PiiCategory::Email));
    }

    #[test]
    fn the_token_is_renamed_in_the_text_and_the_map() {
        let original = "552100554";
        let mut findings = vec![finding_in(original, "552100554", PiiCategory::Phone, "[PHONE_1]")];
        relabel_in_text(&mut findings, original);
        let mut text = "Le SIREN [PHONE_1] est connu.".to_string();
        let mut map = BTreeMap::from([("[PHONE_1]".to_string(), "552100554".to_string())]);
        assert_eq!(rename_tokens(&mut text, &mut map, &findings), 1);
        assert_eq!(text, "Le SIREN [TAX_ID_1] est connu.");
        assert_eq!(map.get("[TAX_ID_1]"), Some(&"552100554".to_string()));
        assert!(!map.contains_key("[PHONE_1]"));
    }

    #[test]
    fn a_token_that_belongs_to_a_real_phone_is_left_alone() {
        let original = "552100554";
        let mut findings = vec![finding_in(original, "552100554", PiiCategory::Phone, "[PHONE_1]")];
        relabel_in_text(&mut findings, original);
        let mut text = "SIREN [PHONE_1], téléphone [PHONE_2].".to_string();
        let mut map = BTreeMap::from([
            ("[PHONE_1]".to_string(), "552100554".to_string()),
            ("[PHONE_2]".to_string(), "0612345678".to_string()),
        ]);
        rename_tokens(&mut text, &mut map, &findings);
        assert_eq!(text, "SIREN [TAX_ID_1], téléphone [PHONE_2].");
        assert!(map.contains_key("[PHONE_2]"));
    }

    #[test]
    fn the_same_value_detected_twice_is_renamed_once() {
        let original = "552100554 552100554";
        let mut findings = vec![
            finding_in(original, "552100554", PiiCategory::Phone, "[PHONE_1]"),
            RedactionFinding {
                start: 10,
                end: 19,
                category: PiiCategory::Phone,
                strategy: xberg::types::redaction::RedactionStrategy::TokenReplace,
                replacement_token: "[PHONE_1]".to_string(),
            },
        ];
        relabel_in_text(&mut findings, original);
        let mut text = "[PHONE_1] et [PHONE_1].".to_string();
        let mut map = BTreeMap::from([("[PHONE_1]".to_string(), "552100554".to_string())]);
        assert_eq!(rename_tokens(&mut text, &mut map, &findings), 1);
        assert_eq!(text, "[TAX_ID_1] et [TAX_ID_1].");
    }

    #[test]
    fn a_token_the_engine_already_issued_as_a_tax_id_is_left_alone() {
        // A custom-pattern finding carries `[TAX_ID_n]` on its own: it stays as it is — and
        // its number stays claimed, so a re-labelled phone finding takes the next one.
        let original = "SIREN 552100554, TVA FR40330302181.";
        let mut findings = vec![
            finding_in(original, "552100554", PiiCategory::Phone, "[PHONE_1]"),
            finding_in(
                original,
                "FR40330302181",
                PiiCategory::Custom("tax_id".to_string()),
                "[TAX_ID_1]",
            ),
        ];
        relabel_in_text(&mut findings, original);
        let mut text = "Le SIREN [PHONE_1], la TVA [TAX_ID_1].".to_string();
        let mut map = BTreeMap::from([
            ("[PHONE_1]".to_string(), "552100554".to_string()),
            ("[TAX_ID_1]".to_string(), "FR40330302181".to_string()),
        ]);
        assert_eq!(rename_tokens(&mut text, &mut map, &findings), 1);
        assert_eq!(text, "Le SIREN [TAX_ID_2], la TVA [TAX_ID_1].");
        assert_eq!(map.get("[TAX_ID_1]"), Some(&"FR40330302181".to_string()));
        assert_eq!(map.get("[TAX_ID_2]"), Some(&"552100554".to_string()));
    }

    #[test]
    fn a_destination_number_already_in_the_text_is_skipped() {
        // `[TAX_ID_1]` dangles in the redacted text (the document itself mentions it), so
        // the re-labelled finding must not share the notation.
        let original = "552100554";
        let mut findings = vec![finding_in(original, "552100554", PiiCategory::Phone, "[PHONE_1]")];
        relabel_in_text(&mut findings, original);
        let mut text = "Renvoi: [TAX_ID_1]. SIREN [PHONE_1]".to_string();
        let mut map = BTreeMap::from([("[PHONE_1]".to_string(), "552100554".to_string())]);
        assert_eq!(rename_tokens(&mut text, &mut map, &findings), 1);
        assert_eq!(text, "Renvoi: [TAX_ID_1]. SIREN [TAX_ID_2]");
        // The dangling occurrence is untouched: the map only claims [TAX_ID_2].
        assert_eq!(map.get("[TAX_ID_2]"), Some(&"552100554".to_string()));
        assert!(!map.contains_key("[TAX_ID_1]"));
    }

    /// A finding whose offsets do not point at a value (used where only the category matters).
    fn finding(category: PiiCategory, value: &str, token: &str) -> RedactionFinding {
        RedactionFinding {
            start: 0,
            end: value.len() as u32,
            category,
            strategy: xberg::types::redaction::RedactionStrategy::TokenReplace,
            replacement_token: token.to_string(),
        }
    }
}
