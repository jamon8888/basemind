//! Checksum validators for EU national IDs, IBAN, and structured PII formats.
//! Pure functions over `&str`; the regex pre-filter lives in [`super::patterns`].

// ─── EU National ID validators ────────────────────────────────────────────────

/// Validates a French NIR (Numéro d'Inscription au Répertoire).
/// Returns true if the 15-digit number passes the MOD-97 checksum.
/// Validates a French NIR (Numéro d'Inscription au Répertoire).
/// 15 chars; Corsica department codes `2A`/`2B` at positions 6–7 are
/// substituted with `19`/`18` before the MOD-97 checksum (per python-stdnum
/// `fr.nir`). A zero remainder yields check digits `97`, not `00`.
pub fn validate_fr_nir(s: &str) -> bool {
    let s: String = s
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_uppercase();
    let chars: Vec<char> = s.chars().collect();
    if chars.len() != 15 || !chars[..5].iter().all(|c| c.is_ascii_digit()) {
        return false;
    }
    let dept: String = chars[5..7].iter().collect();
    let dept = match dept.as_str() {
        "2A" => "19",
        "2B" => "18",
        d if d.chars().all(|c| c.is_ascii_digit()) => {
            if !chars[7..13].iter().all(|c| c.is_ascii_digit()) || !chars[13..15].iter().all(|c| c.is_ascii_digit()) {
                return false;
            }
            d
        }
        _ => return false,
    };
    let first13: String = chars[..5].iter().collect::<String>() + dept + &chars[7..13].iter().collect::<String>();
    let first13_val: u64 = first13.parse().unwrap_or(u64::MAX);
    if first13_val == u64::MAX {
        return false;
    }
    let check = 97 - (first13_val % 97) as u32;
    let last2: u32 = chars[13..15].iter().collect::<String>().parse().unwrap_or(u32::MAX);
    last2 == check
}

/// Validates a Dutch BSN (Burgerservicenummer) using the elf-proef (11-check).
/// Accepts 8-digit (padded with leading zero) or 9-digit numbers.
pub fn validate_nl_bsn(s: &str) -> bool {
    let digits: Vec<u32> = s.chars().filter_map(|c| c.to_digit(10)).collect();
    if digits.len() == 9 {
        bsn_eleven_proef(&digits)
    } else if digits.len() == 8 {
        bsn_eleven_proef(&[0].iter().chain(digits.iter()).cloned().collect::<Vec<_>>()[..])
    } else {
        false
    }
}

fn bsn_eleven_proef(digits: &[u32]) -> bool {
    let sum: i32 = digits
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let w = if i == digits.len() - 1 { -1 } else { 9 - i as i32 };
            *d as i32 * w
        })
        .sum();
    sum % 11 == 0
}

/// Validates a Belgian NISS (Numéro d'Identification de la Sécurité Sociale,
/// Rijksregisternummer). 11 digits: YYMMDD + 3-digit serial + 2-digit check.
/// Checksum per python-stdnum `be.nn`: `97 - (int(n[:-2]) % 97) == int(n[-2:])`,
/// trying both the bare number and the `2`-prefixed variant for post-2000 births.
/// No birth-date validation — the regex is conservative, the checksum filters.
pub fn validate_be_niss(s: &str) -> bool {
    let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() != 11 || digits.chars().all(|c| c == '0') {
        return false;
    }
    let be_check = |n: &str| -> bool {
        let (head, tail) = n.split_at(n.len() - 2);
        let head_val: u64 = head.parse().unwrap_or(u64::MAX);
        let tail_val: u32 = tail.parse().unwrap_or(u32::MAX);
        let check = 97 - (head_val % 97) as u32;
        check == tail_val
    };
    if be_check(&digits) {
        return true;
    }
    // Post-2000 births: the same check against '2' + number (stdnum tries this
    // variant when YY + 2000 <= current year; trying unconditionally is a
    // negligible over-accept for a checksum pre-filter).
    be_check(&format!("2{digits}"))
}

/// Validates an Austrian SVNR (Sozialversicherungsnummer).
/// Returns true if the 10-digit number passes the weighted MOD-11 checksum.
/// The 4th digit is the check digit; weights [3,7,9,5,8,4,2,1,6] are applied
/// to the remaining 9 digits (positions 1-3 and 5-10), and the 10th position
/// (index 3) must equal sum % 11.
pub fn validate_at_svnr(s: &str) -> bool {
    let digits: Vec<u32> = s.chars().filter_map(|c| c.to_digit(10)).collect();
    if digits.len() != 10 {
        return false;
    }
    let check = digits[3];
    let sum = digits[0] * 3
        + digits[1] * 7
        + digits[2] * 9
        + digits[4] * 5
        + digits[5] * 8
        + digits[6] * 4
        + digits[7] * 2
        + digits[8]
        + digits[9] * 6;
    (sum % 11) == check
}

/// Validates an Irish PPS (Personal Public Service number).
/// 7 digits + check letter (8 chars), optionally followed by a second letter
/// (9 chars). Check digit per python-stdnum `ie.vat.calc_check_digit`:
/// alphabet `WABCDEFGHIJKLMNOPQRSTUV`, weights 8..2 over the 7 digits plus
/// `9 * alphabet.index(second_letter)` for 2013-format numbers whose second
/// letter is in `ABH`. Old-format second letters (`WTX`) are ignored.
pub fn validate_ie_pps(s: &str) -> bool {
    const ALPHABET: &str = "WABCDEFGHIJKLMNOPQRSTUV";
    let s: String = s
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect::<String>()
        .to_ascii_uppercase();
    let chars: Vec<char> = s.chars().collect();
    if chars.len() != 8 && chars.len() != 9 {
        return false;
    }
    if !chars[..7].iter().all(|c| c.is_ascii_digit()) {
        return false;
    }
    // First check letter is restricted to A-W (stdnum `pps_re`).
    if !(('A'..='W').contains(&chars[7])) {
        return false;
    }
    let digits: Vec<u32> = chars[..7].iter().filter_map(|c| c.to_digit(10)).collect();
    let mut sum: u32 = digits.iter().enumerate().map(|(i, d)| d * (8 - i as u32)).sum();
    if chars.len() == 9 {
        let second = chars[8];
        if "ABH".contains(second) {
            let idx = ALPHABET.find(second).unwrap_or(0) as u32;
            sum += 9 * idx;
        } else if !"WTX".contains(second) {
            return false;
        }
        // Old-format (WTX): second letter ignored, falls through to 7-digit check.
    }
    let expected = ALPHABET.chars().nth((sum % 23) as usize).unwrap_or('?');
    chars[7] == expected
}

/// Validates a Portuguese NIF (Número de Identificação Fiscal).
/// Returns true if the 9-digit number passes the weighted MOD-11 checksum.
pub fn validate_pt_nif(s: &str) -> bool {
    let digits: Vec<u32> = s.chars().filter_map(|c| c.to_digit(10)).collect();
    if digits.len() != 9 {
        return false;
    }
    let sum = digits[0] * 9
        + digits[1] * 8
        + digits[2] * 7
        + digits[3] * 6
        + digits[4] * 5
        + digits[5] * 4
        + digits[6] * 3
        + digits[7] * 2;
    let remainder = sum % 11;
    let expected = if remainder < 2 { 0 } else { 11 - remainder };
    expected == digits[8]
}

/// Dispatches to the country-specific validator for EU national IDs.
/// Returns true for any label not in the known set (idempotent pass-through).
pub fn validate_eu_national_id(label: &str, value: &str) -> bool {
    match label {
        "national_id_fr" => validate_fr_nir(value),
        "national_id_nl" => validate_nl_bsn(value),
        "national_id_be" => validate_be_niss(value),
        "national_id_at" => validate_at_svnr(value),
        "national_id_ie" => validate_ie_pps(value),
        "national_id_pt" => validate_pt_nif(value),
        _ => true,
    }
}

/// Valid IBAN lengths per country code (ISO 13616 IBAN Registry).
/// Covers all EU/EEA member states and common international codes.
///ponytail: basic HashMap lookup — replace with IBAN registry crate if per-country format validation is needed
static IBAN_LENGTHS: std::sync::LazyLock<std::collections::HashMap<&'static str, usize>> =
    std::sync::LazyLock::new(|| {
        let mut m = std::collections::HashMap::new();
        // EU/EEA
        m.insert("AT", 20); // Austria
        m.insert("BE", 16); // Belgium
        m.insert("BG", 22); // Bulgaria
        m.insert("HR", 21); // Croatia
        m.insert("CY", 28); // Cyprus
        m.insert("CZ", 24); // Czech Republic
        m.insert("DK", 18); // Denmark (incl. FO, GL)
        m.insert("EE", 20); // Estonia
        m.insert("FI", 18); // Finland (incl. AX)
        m.insert("FR", 27); // France (incl. GF, GP, MQ, RE, YT, NC, PF, TF, BL, MF, PM, WF)
        m.insert("DE", 22); // Germany
        m.insert("GR", 27); // Greece
        m.insert("HU", 28); // Hungary
        m.insert("IE", 22); // Ireland
        m.insert("IS", 26); // Iceland
        m.insert("IT", 27); // Italy (incl. SM, VA)
        m.insert("LV", 21); // Latvia
        m.insert("LI", 21); // Liechtenstein
        m.insert("LT", 20); // Lithuania
        m.insert("LU", 20); // Luxembourg
        m.insert("MT", 31); // Malta
        m.insert("MC", 27); // Monaco
        m.insert("NL", 18); // Netherlands (incl. AW, CW, SX)
        m.insert("NO", 15); // Norway (incl. SJ, BV)
        m.insert("PL", 28); // Poland
        m.insert("PT", 25); // Portugal (incl. MH, PW)
        m.insert("RO", 24); // Romania
        m.insert("SM", 27); // San Marino
        m.insert("SK", 24); // Slovakia
        m.insert("SI", 19); // Slovenia
        m.insert("ES", 24); // Spain
        m.insert("SE", 24); // Sweden
        m.insert("CH", 21); // Switzerland
        m.insert("GB", 22); // United Kingdom (incl. IM, JE, GG)
        m.insert("AD", 24); // Andorra
        m.insert("AL", 28); // Albania
        m.insert("MK", 19); // North Macedonia
        m.insert("XK", 20); // Kosovo
        m
    });

/// Validates an IBAN using the MOD-97 checksum (ISO 13616).
/// Returns true if the country code is registered, the length is correct, and the check digits are valid.
pub fn validate_iban(iban: &str) -> bool {
    let iban = iban.chars().filter(|c| !c.is_whitespace()).collect::<String>();
    if !(5..=34).contains(&iban.len()) {
        return false;
    }
    if !iban.chars().all(|c| c.is_ascii_alphanumeric()) {
        return false;
    }
    let country = &iban[..2];
    if !iban.chars().take(2).all(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    if !iban.chars().skip(2).take(2).all(|c| c.is_ascii_digit()) {
        return false;
    }
    if let Some(&expected_len) = IBAN_LENGTHS.get(country) {
        if iban.len() != expected_len {
            return false;
        }
    } else {
        return false;
    }
    let normalized = iban.to_ascii_uppercase();
    let acc = normalized
        .as_bytes()
        .iter()
        .cycle()
        .skip(4)
        .take(normalized.len())
        .fold(0_u64, |acc, &c| {
            if c.is_ascii_digit() {
                (acc * 10 + u64::from(c - b'0')) % 97
            } else {
                (acc * 100 + u64::from(c - b'A' + 10)) % 97
            }
        });
    acc == 1
}
