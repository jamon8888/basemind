//! Bounded extraction of identifier-shaped legal citations for the exact retrieval lane.

pub(crate) fn extract_citations(text: &str, max_per_chunk: usize) -> Vec<String> {
    if max_per_chunk == 0 {
        return Vec::new();
    }
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() && out.len() < max_per_chunk {
        let (start, marker_len) = if i + 2 <= bytes.len() && bytes[i..i + 2] == [0xC2, 0xA7] {
            (i, 2)
        } else if i + 3 <= bytes.len()
            && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric())
            && bytes[i..i + 3].eq_ignore_ascii_case(b"no.")
        {
            (i, 3)
        } else {
            i += 1;
            continue;
        };
        let mut end = start + marker_len;
        while end < bytes.len() && bytes[end].is_ascii_whitespace() {
            end += 1;
        }
        let digit_start = end;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        if end == digit_start {
            i = start + marker_len;
            continue;
        }
        while end < bytes.len() {
            let c = bytes[end];
            if c.is_ascii_alphanumeric() || matches!(c, b'(' | b')' | b'-' | b'/' | b'.' | b':') {
                end += 1;
            } else {
                break;
            }
        }
        let value = text[start..end].split_whitespace().collect::<Vec<_>>().join(" ");
        if !out
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(&value))
        {
            out.push(value);
        }
        i = end;
    }
    out
}

pub(crate) fn filter_self_references(citations: &mut Vec<String>, excluded: &[String]) {
    citations.retain(|citation| !excluded.iter().any(|value| value.eq_ignore_ascii_case(citation)));
}

#[cfg(test)]
mod tests {
    use super::extract_citations;
    #[test]
    fn extracts_statute_and_docket_references() {
        assert_eq!(
            extract_citations("under § 362(a)(1), see No. 2-24-1234", 8),
            vec!["§ 362(a)(1)", "No. 2-24-1234"]
        );
    }
    #[test]
    fn dedupes_and_caps() {
        assert_eq!(extract_citations("§ 362 § 362 No. 3", 8), vec!["§ 362", "No. 3"]);
        assert_eq!(extract_citations("§ 362 § 365 No. 3", 1), vec!["§ 362"]);
    }
    #[test]
    fn rejects_markers_without_digits() {
        assert!(extract_citations("No. other text § (a)", 8).is_empty());
    }

    #[test]
    fn no_marker_requires_word_boundary() {
        assert_eq!(extract_citations("Reno. 2 is not a docket; No. 3 is", 8), vec!["No. 3"]);
    }
}
