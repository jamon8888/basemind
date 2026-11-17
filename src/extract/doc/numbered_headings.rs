//! Bounded numbered-heading recovery for large documents that use linear chunking.

const MAX_HEADING_TEXT: usize = 512;

pub(super) fn collect_numbered_headings(source: &str) -> Vec<(usize, usize, String)> {
    let mut headings = Vec::new();
    let mut offset = 0usize;
    for raw_line in source.split_inclusive('\n') {
        let line_start = offset;
        offset += raw_line.len();
        let trimmed = raw_line.trim_end_matches(['\n', '\r']).trim();
        let Some(separator) = trimmed.find(char::is_whitespace) else {
            continue;
        };
        let (raw_number, caption) = trimmed.split_at(separator);
        let number = raw_number.trim_end_matches('.');
        let roman = is_roman_numeral(number);
        let dotted_arabic =
            number.split_once('.').is_some() && number.split('.').all(|part| part.parse::<u32>().is_ok());
        let caption = caption.trim();
        let single_arabic = number.parse::<u32>().is_ok() && caption.chars().all(|c| !c.is_lowercase());
        if !(roman || dotted_arabic || single_arabic) || caption.is_empty() || caption.len() > MAX_HEADING_TEXT {
            continue;
        }
        let level = if dotted_arabic { number.split('.').count() } else { 1 };
        headings.push((line_start, level, format!("{raw_number} {caption}")));
    }
    headings
}

pub(super) fn numbered_heading_path(headings: &[(usize, usize, String)], byte_start: usize, byte_end: usize) -> String {
    let mut stack: Vec<(usize, String)> = Vec::new();
    for (line_start, level, text) in headings {
        if *line_start > byte_start || *line_start >= byte_end {
            break;
        }
        while stack.last().is_some_and(|(parent_level, _)| *parent_level >= *level) {
            stack.pop();
        }
        stack.push((*level, text.clone()));
    }
    stack.into_iter().map(|(_, text)| text).collect::<Vec<_>>().join(" > ")
}

fn is_roman_numeral(token: &str) -> bool {
    let upper = token.to_ascii_uppercase();
    if upper.is_empty()
        || !upper
            .chars()
            .all(|c| matches!(c, 'I' | 'V' | 'X' | 'L' | 'C' | 'D' | 'M'))
    {
        return false;
    }
    let mut value = 0u32;
    let mut previous = 0u32;
    for ch in upper.chars().rev() {
        let current = match ch {
            'I' => 1,
            'V' => 5,
            'X' => 10,
            'L' => 50,
            'C' => 100,
            'D' => 500,
            'M' => 1000,
            _ => return false,
        };
        if current < previous {
            value -= current;
        } else {
            value += current;
            previous = current;
        }
    }
    value > 0 && format_roman(value) == upper
}

fn format_roman(mut value: u32) -> String {
    let mut out = String::new();
    for (unit, glyph) in [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ] {
        while value >= unit {
            out.push_str(glyph);
            value -= unit;
        }
    }
    out
}
