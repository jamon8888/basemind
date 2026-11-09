use std::time::{Duration, Instant};

use xberg::ExtractInput;

pub(super) const FILE_EXTRACTION_TIMEOUT: Duration = Duration::from_secs(120);

pub(super) async fn extract_file_with_deadline(
    input: ExtractInput,
    config: &xberg::core::config::ExtractionConfig,
    deadline: Instant,
) -> Result<xberg::ExtractionResult, String> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err("xberg file extraction shared deadline exhausted before extraction".to_string());
    }
    match tokio::time::timeout(remaining, xberg::extract(input, config)).await {
        Ok(Ok(extraction)) => Ok(extraction),
        Ok(Err(_)) => Err("xberg file extraction failed".to_string()),
        Err(_) => {
            let elapsed = FILE_EXTRACTION_TIMEOUT.saturating_sub(deadline.saturating_duration_since(Instant::now()));
            Err(format!(
                "xberg file extraction timed out after {:?} of {:?} shared budget",
                elapsed, FILE_EXTRACTION_TIMEOUT
            ))
        }
    }
}
