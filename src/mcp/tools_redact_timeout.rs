use std::time::{Duration, Instant};

use xberg::ExtractInput;

/// Default shared budget for extracting one file (first attempt plus the OCR retry).
pub(super) const FILE_EXTRACTION_TIMEOUT: Duration = Duration::from_secs(120);
/// Overrides [`FILE_EXTRACTION_TIMEOUT`], in seconds. A caller with its own, longer deadline
/// (a gateway that allows several minutes for a long PDF) raises it so basemind does not stop
/// first. Unset, unparsable, zero or above [`MAX_FILE_EXTRACTION_TIMEOUT_SECS`]: the default.
pub(crate) const FILE_EXTRACTION_TIMEOUT_ENV: &str = "BASEMIND_FILE_EXTRACTION_TIMEOUT_SECS";
const MAX_FILE_EXTRACTION_TIMEOUT_SECS: u64 = 3_600;

/// The shared file-extraction budget, read from [`FILE_EXTRACTION_TIMEOUT_ENV`].
pub(super) fn file_extraction_timeout() -> Duration {
    parse_file_extraction_timeout(std::env::var(FILE_EXTRACTION_TIMEOUT_ENV).ok().as_deref())
}

fn parse_file_extraction_timeout(raw: Option<&str>) -> Duration {
    raw.and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|secs| (1..=MAX_FILE_EXTRACTION_TIMEOUT_SECS).contains(secs))
        .map_or(FILE_EXTRACTION_TIMEOUT, Duration::from_secs)
}

pub(super) async fn extract_file_with_deadline(
    input: ExtractInput,
    config: &xberg::core::config::ExtractionConfig,
    budget_start: Instant,
    deadline: Instant,
) -> Result<xberg::ExtractionResult, String> {
    let attempt_begin = Instant::now();
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err("xberg file extraction shared deadline exhausted before extraction".to_string());
    }
    match tokio::time::timeout(remaining, xberg::extract(input, config)).await {
        Ok(Ok(extraction)) => Ok(extraction),
        Ok(Err(_)) => Err("xberg file extraction failed".to_string()),
        Err(_) => Err(format!(
            "xberg file extraction timed out after {:?} of {:?} shared budget (this attempt: {:?})",
            budget_start.elapsed(),
            deadline.saturating_duration_since(budget_start),
            attempt_begin.elapsed(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::{FILE_EXTRACTION_TIMEOUT, parse_file_extraction_timeout};
    use std::time::Duration;

    #[test]
    fn file_extraction_timeout_reads_a_bounded_override() {
        assert_eq!(parse_file_extraction_timeout(None), FILE_EXTRACTION_TIMEOUT);
        assert_eq!(parse_file_extraction_timeout(Some(" 300 ")), Duration::from_secs(300));
        assert_eq!(parse_file_extraction_timeout(Some("3600")), Duration::from_secs(3_600));
        for rejected in ["0", "3601", "-5", "trois cents", ""] {
            assert_eq!(
                parse_file_extraction_timeout(Some(rejected)),
                FILE_EXTRACTION_TIMEOUT,
                "{rejected:?}"
            );
        }
    }
}
