//! French company identifiers: SIREN (the company) and SIRET (the company + its establishment).
//!
//! Both are Luhn-checked digit runs, and both are what a French legal file carries when it
//! names the company it is about. The detection engine files any 7-to-15 digit run as a
//! phone number, so a SIREN arrives under the `phone` label — and a phone number is not the
//! category a legal tool may pass on in the clear (a SIREN is: it is the identifier BODACC
//! is queried with). These validators let the engine's own finding be re-labelled `tax_id`
//! when the run it matched is in fact a SIREN or a SIRET.
//!
//! The checksum is the filter: a nine-digit run passes the Luhn test by chance roughly one
//! time in ten, and only a run the file itself presents as a company identifier (nine or
//! fourteen digits, optionally grouped) is accepted here.

/// The label a French company identifier carries once it is recognised.
pub const TAX_ID_LABEL: &str = "tax_id";

/// Validates a SIREN: nine digits, Luhn checksum.
///
/// Accepts the grouped forms a document uses (`552 100 554`, `552.100.554`): the
/// separators are stripped before the checksum runs.
pub fn validate_fr_siren(value: &str) -> bool {
    match digits_of(value) {
        Some(digits) => digits.len() == 9 && is_plausible_run(&digits) && luhn(&digits),
        None => false,
    }
}

/// Validates a SIRET: fourteen digits (a SIREN plus the establishment's five-digit NIC),
/// Luhn checksum over the whole run.
pub fn validate_fr_siret(value: &str) -> bool {
    match digits_of(value) {
        Some(digits) => digits.len() == 14 && is_plausible_run(&digits) && luhn(&digits),
        None => false,
    }
}

/// True for a run that could be a real identifier at all.
///
/// An all-zero run satisfies Luhn by construction (`0` is its own check digit), and no
/// identifier is all zeros — the checksum alone would admit it.
fn is_plausible_run(digits: &[u32]) -> bool {
    digits.iter().any(|&digit| digit != 0)
}

/// True when `value` is a SIREN or a SIRET — the two French company identifiers.
///
/// This is the test the re-labeling applies: a run the engine filed as a phone number that
/// turns out to be a company identifier is a company identifier.
pub fn is_fr_company_id(value: &str) -> bool {
    validate_fr_siren(value) || validate_fr_siret(value)
}

/// Validates the numeric form of a French intra-community VAT number.
///
/// France prefixes the nine-digit SIREN with `FR` and a two-digit key. The
/// key is `12 + 3 × (SIREN mod 97)`, reduced modulo 97. The alphanumeric key
/// variants used by some legacy registrations cannot be verified by this
/// arithmetic form and are conservatively rejected here.
pub fn validate_fr_vat(value: &str) -> bool {
    let compact: String = value
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_uppercase();
    if compact.len() != 13 || !compact.is_ascii() || !compact.starts_with("FR") {
        return false;
    }
    let key = &compact[2..4];
    let siren = &compact[4..];
    if !key.bytes().all(|byte| byte.is_ascii_digit()) || !validate_fr_siren(siren) {
        return false;
    }
    let Ok(siren_value) = siren.parse::<u64>() else {
        return false;
    };
    let expected = (12 + 3 * (siren_value % 97)) % 97;
    key.parse::<u64>().ok() == Some(expected)
}

/// True when a model `tax_id` span has a verifiable French company/tax shape.
pub fn is_fr_tax_id(value: &str) -> bool {
    is_fr_company_id(value) || validate_fr_vat(value)
}

/// The digits of `value`, with the separators French documents use removed, or `None` when
/// `value` holds anything else.
///
/// Only the separators are removed: a letter anywhere means the run is not an identifier on
/// its own (`SIREN 552100554` is a label followed by a number, not a number), so the value
/// is refused rather than reduced to its digits.
fn digits_of(value: &str) -> Option<Vec<u32>> {
    let mut digits = Vec::with_capacity(value.len());
    for c in value.chars() {
        if let Some(digit) = c.to_digit(10) {
            digits.push(digit);
        } else if !matches!(c, ' ' | '\u{00a0}' | '.' | '-' | '\u{2013}' | '\u{2014}') {
            return None;
        }
    }
    Some(digits)
}

/// The Luhn checksum: the run passes when the doubled-every-second-digit sum is a multiple
/// of ten. The rightmost digit is the check digit and is not doubled.
fn luhn(digits: &[u32]) -> bool {
    let sum: u32 = digits
        .iter()
        .rev()
        .enumerate()
        .map(|(index, digit)| {
            let mut value = *digit;
            if !index.is_multiple_of(2) {
                value *= 2;
                if value > 9 {
                    value -= 9;
                }
            }
            value
        })
        .sum();
    sum.is_multiple_of(10)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_known_siren_passes() {
        // Société Générale and Crédit Agricole, both published SIRENs.
        assert!(validate_fr_siren("552100554"));
        assert!(validate_fr_siren("552 100 554"));
        assert!(validate_fr_siren("552.100.554"));
        assert!(validate_fr_siren("732829320"));
    }

    #[test]
    fn a_siren_with_a_wrong_check_digit_fails() {
        assert!(!validate_fr_siren("552100555"));
        assert!(!validate_fr_siren("123456789"));
        assert!(!validate_fr_siren("000000000"));
    }

    #[test]
    fn a_siret_passes_and_its_siren_part_alone_does_not() {
        // A SIRET is fourteen digits: the SIREN plus the establishment's NIC.
        assert!(validate_fr_siret("55210055400013"));
        assert!(!validate_fr_siret("55210055400014"));
        // Fourteen digits is not a SIREN.
        assert!(!validate_fr_siren("55210055400013"));
    }

    #[test]
    fn a_phone_number_is_not_a_company_identifier() {
        assert!(!is_fr_company_id("0612345678"));
        assert!(!is_fr_company_id("+33 6 12 34 56 78"));
        assert!(!is_fr_company_id("0142424242"));
    }

    #[test]
    fn a_nine_digit_run_that_fails_the_checksum_is_not_an_identifier() {
        assert!(!is_fr_company_id("123456789"));
        assert!(!is_fr_company_id("987654321"));
    }

    #[test]
    fn letters_make_the_value_not_an_identifier() {
        assert!(!is_fr_company_id("55210055X"));
        assert!(!is_fr_company_id("SIREN 552100554"));
    }

    #[test]
    fn an_empty_value_is_not_an_identifier() {
        assert!(!is_fr_company_id(""));
        assert!(!is_fr_company_id("   "));
    }

    #[test]
    fn a_numeric_french_vat_number_passes() {
        assert!(validate_fr_vat("FR96552100554"));
        assert!(!validate_fr_vat("FR96552100555"));
    }

    #[test]
    fn a_tax_id_accepts_siren_siret_or_vat() {
        assert!(is_fr_tax_id("552100554"));
        assert!(is_fr_tax_id("55210055400013"));
        assert!(is_fr_tax_id("FR96552100554"));
        assert!(!is_fr_tax_id("F-2025-1142"));
    }
}
