//! Server→client progress notifications (`notifications/progress`).
//!
//! Best-effort and fire-and-forget — a delivery error is logged via `tracing` and swallowed so a
//! chatty client connection can never fail a tool call. Upstream deprecated MCP logging
//! (SEP-2577) and stopped advertising it; this fork keeps the `logging/setLevel` handler, which
//! records the client's minimum severity on `super::ServerState::log_level` as an ordinal (see
//! [`level_ordinal`]), and the rescan path emits a threshold-gated `rescan_complete` notification
//! through [`emit_log`].

use rmcp::Peer;
use rmcp::RoleServer;
use rmcp::model::ProgressNotificationParam;
#[allow(deprecated)]
use rmcp::model::{LoggingLevel, LoggingMessageNotificationParam};
use serde_json::Value;
use std::sync::atomic::{AtomicU8, Ordering};

/// Severity ordinal (RFC 5424 order: lower = more verbose). `logging/setLevel L` means "send me
/// messages at severity `L` and above", so a message is emitted when its ordinal `>=` the stored
/// threshold.
#[allow(deprecated)]
pub(super) fn level_ordinal(level: LoggingLevel) -> u8 {
    match level {
        LoggingLevel::Debug => 0,
        LoggingLevel::Info => 1,
        LoggingLevel::Notice => 2,
        LoggingLevel::Warning => 3,
        LoggingLevel::Error => 4,
        LoggingLevel::Critical => 5,
        LoggingLevel::Alert => 6,
        LoggingLevel::Emergency => 7,
    }
}

/// Default logging threshold before the client calls `logging/setLevel`: `Info` and above.
pub(super) const DEFAULT_LOG_ORDINAL: u8 = 1;

/// True when a message at `level` should be sent given the client's current threshold.
#[allow(deprecated)]
pub(super) fn should_log(threshold: &AtomicU8, level: LoggingLevel) -> bool {
    level_ordinal(level) >= threshold.load(Ordering::Relaxed)
}

/// Emit a logging notification if `level` clears the client's threshold. Best-effort.
#[allow(deprecated)]
pub(super) async fn emit_log(
    peer: &Peer<RoleServer>,
    threshold: &AtomicU8,
    level: LoggingLevel,
    logger: &'static str,
    data: Value,
) {
    if !should_log(threshold, level) {
        return;
    }
    if let Err(error) = peer
        .notify_logging_message(LoggingMessageNotificationParam::new(level, data).with_logger(logger))
        .await
    {
        tracing::debug!(?error, "logging notification dropped");
    }
}

/// Emit a progress notification for `token`. Best-effort; only called when the client supplied a
/// progress token on the request.
pub(super) async fn emit_progress(
    peer: &Peer<RoleServer>,
    token: rmcp::model::ProgressToken,
    progress: f64,
    total: Option<f64>,
    message: impl Into<String>,
) {
    let message: String = message.into();
    // Inside a task-offloaded call, mirror the message onto the task's `statusMessage` so a client
    // polling `tasks/get` sees what a progress-token subscriber sees.
    let _ = super::tasks::CURRENT_TASK.try_with(|task| task.set_status_message(message.clone()));
    // `ProgressNotificationParam` is #[non_exhaustive] in rmcp 3.x; build it via the constructor.
    let mut param = ProgressNotificationParam::new(token, progress).with_message(message);
    if let Some(total) = total {
        param = param.with_total(total);
    }
    if let Err(error) = peer.notify_progress(param).await {
        tracing::debug!(?error, "progress notification dropped");
    }
}
