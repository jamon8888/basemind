//! The case dictionary: the values a lawyer renamed, and the code names that stand in for them.
//!
//! A case file carries names the parties do not want spelled out. The lawyer picks a code
//! name for each of them (« Dupont » becomes « Orme ») and adds the terms that belong to the
//! file alone (« Dossier Orion », an address). Two things then have to hold:
//!
//! 1. **The real value never reaches the agent.** It is replaced by its code name — not by a
//!    token, because the code name is the name the file is *meant* to carry.
//! 2. **The term is recognised even when the detection model does not find it.** A term the
//!    model missed is still in the text; the dictionary replaces it directly.
//!
//! The dictionary is therefore an equivalence class: every member of it (the real value and
//! the code name) is rendered as the code name. That also repairs the other direction — a
//! code name the model *did* redact (« Orme » read as a place name) is put back, because the
//! code name is the sanctioned form.
//!
//! The rehydration map is left alone: it still maps each token to the value it replaced, so
//! the gateway stays reversible and keeps the only copy of the true value it needs.

use std::collections::{BTreeMap, HashMap, HashSet};

use regex::Regex;

/// One entry of a case dictionary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pseudonym {
    /// The value that must not reach the agent: the real name, the project's name, an address.
    pub value: String,
    /// The code name that stands in for it.
    pub code: String,
}

impl Pseudonym {
    /// Build an entry, dropping a pair whose value or code name is empty.
    pub fn new(value: impl Into<String>, code: impl Into<String>) -> Option<Self> {
        let value = value.into();
        let code = code.into();
        if value.trim().is_empty() || code.trim().is_empty() {
            return None;
        }
        Some(Self { value, code })
    }
}

/// What the dictionary did, so a caller can record it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Applied {
    /// Tokens the engine had produced for a dictionary value, replaced by its code name.
    pub tokens: Vec<(String, String)>,
    /// Dictionary values the engine had not detected, replaced in the text as they stood.
    pub literals: Vec<(String, String)>,
}

impl Applied {
    /// True when the dictionary changed nothing.
    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty() && self.literals.is_empty()
    }
}

/// Apply the dictionary to a redacted text.
///
/// `map` is the rehydration map the engine produced (token → the value it replaced) and
/// `text` the text it produced. The returned text carries the code names; the map is not
/// modified, so the caller keeps a reversible record of the true values.
pub fn apply(text: &str, map: &BTreeMap<String, String>, entries: &[Pseudonym]) -> (String, Applied) {
    apply_with_counts(text, map, entries, &HashMap::new())
}

/// [`apply`], bounded: `token_counts` carries, per engine token, how many findings
/// produced it. The token sweep replaces exactly that many occurrences — in reading
/// order — rather than every occurrence in the text. A document that quotes the token
/// notation itself ("then read [PERSON_1] again") carries occurrences the engine did
/// not write; an unbounded sweep would rewrite that authored text into the code name
/// with no detection behind it. An authored extra is left as the bare token: still
/// masked, and rehydrated through the map exactly like every occurrence of it. A token
/// absent from `token_counts` sweeps unbounded, as before.
pub fn apply_with_counts(
    text: &str,
    map: &BTreeMap<String, String>,
    entries: &[Pseudonym],
    token_counts: &HashMap<String, usize>,
) -> (String, Applied) {
    let mut usable: Vec<&Pseudonym> = entries
        .iter()
        .filter(|entry| !entry.value.trim().is_empty() && !entry.code.trim().is_empty())
        .collect();
    // Longest value first: with (« Jean Dupont », « Orme ») and (« Dupont », « Chêne »)
    // given the other way round, the shorter value would rewrite the inside of the longer
    // one (« Jean Chêne »), and the « Jean Dupont » entry would then never match — leaving
    // part of what the dictionary hides standing in the text. The stable sort keeps the
    // caller's own order among entries of equal length.
    usable.sort_by_key(|entry| std::cmp::Reverse(entry.value.chars().count()));
    if usable.is_empty() {
        return (text.to_string(), Applied::default());
    }

    // Every member of the equivalence class maps to the code name. A code name is registered
    // first and wins: it is the sanctioned form, so it is never rewritten into something else.
    let mut lookup: HashMap<String, String> = HashMap::new();
    for entry in &usable {
        lookup
            .entry(normalize(&entry.code))
            .or_insert_with(|| entry.code.clone());
    }
    for entry in &usable {
        let key = normalize(&entry.value);
        if lookup.contains_key(&key) {
            // The value is also a code name: it is protected, not rewritten.
            continue;
        }
        lookup.insert(key, entry.code.clone());
    }
    // The code names that are protected from the literal pass.
    let protected: HashSet<String> = usable.iter().map(|entry| normalize(&entry.code)).collect();

    let mut applied = Applied::default();
    let mut out = text.to_string();

    // The engine found it: the token becomes the code name.
    for (token, original) in map {
        let Some(code) = lookup.get(&normalize(original)) else {
            continue;
        };
        // Only a bracketed token is replaced as a literal: a bare word could sit inside
        // unrelated text, and the engine's token format is `[TYPE_N]`.
        if !is_bracketed_token(token) {
            continue;
        }
        // The bounded sweep (see [`apply_with_counts`]): only the occurrences the
        // engine produced carry the code name.
        let expected = token_counts
            .get(token.as_str())
            .copied()
            .map_or(usize::MAX, |count| count.max(1));
        out = out.replacen(token.as_str(), code, expected);
        applied.tokens.push((token.clone(), code.clone()));
    }

    // The engine did not find it: the value still stands in the text, and the dictionary
    // replaces it there.
    for entry in &usable {
        let key = normalize(&entry.value);
        if protected.contains(&key) {
            continue;
        }
        // A code name that contains its own value would be rewritten twice ("Orion" →
        // "Orion Prime" → "Orion Prime Prime"). The dictionary is ambiguous there; the value
        // wins and the code name is left alone.
        if normalize(&entry.code).contains(&key) {
            continue;
        }
        let Some(pattern) = literal_pattern(&entry.value) else {
            continue;
        };
        let replaced = pattern.replace_all(&out, regex::NoExpand(&entry.code)).into_owned();
        if replaced != out {
            applied.literals.push((entry.value.clone(), entry.code.clone()));
            out = replaced;
        }
    }

    (out, applied)
}

/// The key a value is looked up under: case and surrounding space do not distinguish terms.
fn normalize(value: &str) -> String {
    value.trim().to_lowercase()
}

/// True for the engine's token shape (`[TYPE_N]`).
fn is_bracketed_token(token: &str) -> bool {
    token.starts_with('[') && token.ends_with(']') && token.len() > 2
}

/// A boundary-aware, case-insensitive pattern for a literal value.
///
/// The boundary is emitted only at the edges that start or end a word character: a value
/// that starts with punctuation («Maître Dupont») has no word boundary there, and one that
/// ends with a letter does.
fn literal_pattern(value: &str) -> Option<Regex> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let escaped = regex::escape(trimmed);
    let start = if trimmed.starts_with(is_word_char) { r"\b" } else { "" };
    let end = if trimmed.ends_with(is_word_char) { r"\b" } else { "" };
    Regex::new(&format!("(?i){start}{escaped}{end}")).ok()
}

/// Mirrors the regex crate's `\w` (word character) definition.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(token, value)| (token.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn a_detected_value_becomes_its_code_name() {
        let entries = vec![Pseudonym::new("Dupont", "Orme").unwrap()];
        let (text, applied) = apply("Me [PERSON_1] a signé.", &map(&[("[PERSON_1]", "Dupont")]), &entries);
        assert_eq!(text, "Me Orme a signé.");
        assert_eq!(applied.tokens, vec![("[PERSON_1]".to_string(), "Orme".to_string())]);
    }

    #[test]
    fn a_value_the_model_missed_is_replaced_all_the_same() {
        let entries = vec![Pseudonym::new("Dossier Orion", "Dossier Onyx").unwrap()];
        let (text, applied) = apply("Le Dossier Orion débute.", &map(&[]), &entries);
        assert_eq!(text, "Le Dossier Onyx débute.");
        assert_eq!(
            applied.literals,
            vec![("Dossier Orion".to_string(), "Dossier Onyx".to_string())]
        );
    }

    #[test]
    fn a_code_name_the_model_redacted_is_put_back() {
        let entries = vec![Pseudonym::new("Dupont", "Orme").unwrap()];
        let (text, applied) = apply(
            "Le [LOCATION_1] habite ici.",
            &map(&[("[LOCATION_1]", "Orme")]),
            &entries,
        );
        assert_eq!(text, "Le Orme habite ici.");
        assert_eq!(applied.tokens, vec![("[LOCATION_1]".to_string(), "Orme".to_string())]);
    }

    #[test]
    fn the_match_is_case_insensitive_and_whole_word() {
        let entries = vec![Pseudonym::new("Dupont", "Orme").unwrap()];
        let (text, _) = apply("dupont a signé, pas Dupontel.", &map(&[]), &entries);
        assert_eq!(text, "Orme a signé, pas Dupontel.");
    }

    #[test]
    fn a_value_inside_a_longer_word_is_left_alone() {
        let entries = vec![Pseudonym::new("Orion", "Onyx").unwrap()];
        let (text, _) = apply("Orionien est un mot.", &map(&[]), &entries);
        assert_eq!(text, "Orionien est un mot.");
    }

    #[test]
    fn a_value_that_is_a_fragment_of_a_token_does_not_corrupt_the_token() {
        // A token body (`ORION_1`) is bracketed word-to-underscore: a \b-boundary literal
        // can never match inside it, so the token survives while the bare value still falls.
        let entries = vec![Pseudonym::new("Orion", "Onyx").unwrap()];
        let (text, _) = apply("Le [ORION_1] porte le nom Orion.", &map(&[]), &entries);
        assert_eq!(text, "Le [ORION_1] porte le nom Onyx.");
    }

    #[test]
    fn a_longer_value_is_replaced_before_a_fragment_of_it() {
        // « Jean Dupont » must fall to its own code name, not be rewritten inside-out by
        // the shorter « Dupont » entry that shares the surname.
        let entries = vec![
            Pseudonym::new("Jean Dupont", "Orme").unwrap(),
            Pseudonym::new("Dupont", "Chêne").unwrap(),
        ];
        let (text, _) = apply("Jean Dupont et Dupont sont là.", &map(&[]), &entries);
        assert_eq!(text, "Orme et Chêne sont là.");
    }

    #[test]
    fn the_resolution_of_overlapping_values_does_not_depend_on_the_callers_order() {
        let entries = vec![
            Pseudonym::new("Dupont", "Chêne").unwrap(),
            Pseudonym::new("Jean Dupont", "Orme").unwrap(),
        ];
        let (text, _) = apply("Jean Dupont et Dupont sont là.", &map(&[]), &entries);
        assert_eq!(text, "Orme et Chêne sont là.");
    }

    #[test]
    fn a_code_name_is_never_rewritten_into_another_code_name() {
        let entries = vec![
            Pseudonym::new("Dupont", "Orme").unwrap(),
            Pseudonym::new("Orme", "Chêne").unwrap(),
        ];
        let (text, _) = apply("Orme est là.", &map(&[]), &entries);
        assert_eq!(text, "Orme est là.");
    }

    #[test]
    fn an_empty_dictionary_changes_nothing() {
        let (text, applied) = apply("Rien à changer.", &map(&[]), &[]);
        assert_eq!(text, "Rien à changer.");
        assert!(applied.is_empty());
    }

    #[test]
    fn a_token_that_is_not_bracketed_is_not_replaced_as_a_literal() {
        let entries = vec![Pseudonym::new("Dupont", "Orme").unwrap()];
        let (text, applied) = apply("Dupont signe.", &map(&[("Dupont", "Dupont")]), &entries);
        assert_eq!(text, "Orme signe.");
        assert!(applied.tokens.is_empty());
    }

    #[test]
    fn a_pseudonym_without_a_value_or_a_code_is_dropped() {
        assert!(Pseudonym::new("", "Orme").is_none());
        assert!(Pseudonym::new("Dupont", "  ").is_none());
        assert!(Pseudonym::new("Dupont", "Orme").is_some());
    }
}
