//! Error types for the crate.

use std::path::PathBuf;

/// Errors produced by the bridge.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The `FMBridge` helper binary could not be found on disk.
    #[error(
        "FMBridge binary not found at {0}; build it with `swift build -c release --package-path swift` or set FM_BRIDGE_BIN"
    )]
    BinaryNotFound(PathBuf),

    /// The helper binary could not be launched.
    #[error("failed to spawn the FMBridge process at {path}")]
    Spawn {
        /// Path we attempted to execute.
        path: PathBuf,
        /// Underlying OS error.
        #[source]
        source: std::io::Error,
    },

    /// An I/O error occurred while talking to the helper process.
    #[error("i/o error while communicating with FMBridge")]
    Io(#[from] std::io::Error),

    /// A payload could not be serialized or deserialized.
    #[error("could not encode or decode the wire payload")]
    Json(#[from] serde_json::Error),

    /// Apple Intelligence is not usable on this machine right now.
    ///
    /// This is a configuration/hardware problem, not a bug in the caller's
    /// request: the device may be ineligible, Apple Intelligence may be
    /// switched off, or the model may still be downloading.
    #[error("Apple Intelligence is unavailable: {0}")]
    ModelUnavailable(String),

    /// The request was rejected before generation started.
    #[error("invalid request: {0}")]
    BadRequest(String),

    /// The supplied [`Schema`](crate::Schema) could not be validated.
    #[error("invalid schema: {0}")]
    InvalidSchema(String),

    /// Safety guardrails blocked the prompt or the response.
    #[error("blocked by safety guardrails: {0}")]
    GuardrailViolation(String),

    /// The prompt plus the response exceeded the model's context window.
    #[error("context window exceeded: {0}")]
    ContextExceeded(String),

    /// Generation failed for some other model-side reason.
    #[error("generation failed: {0}")]
    Generation(String),

    /// The helper spoke something we could not interpret.
    #[error("protocol error: {0}")]
    Protocol(String),

    /// The helper exited without completing the response.
    #[error("FMBridge exited unexpectedly (status {status}){}", format_stderr(.stderr))]
    ProcessFailed {
        /// Exit status description.
        status: String,
        /// Whatever the helper wrote to stderr.
        stderr: String,
    },

    /// The request exceeded the configured timeout.
    #[error("timed out after {0:?} waiting for FMBridge")]
    Timeout(std::time::Duration),
}

fn format_stderr(stderr: &str) -> String {
    let trimmed = stderr.trim();
    if trimmed.is_empty() {
        String::new()
    } else {
        format!(": {trimmed}")
    }
}

impl Error {
    /// Builds the appropriate variant from the helper's machine-readable code.
    pub(crate) fn from_code(code: &str, message: String) -> Self {
        match code {
            "model_unavailable" => Error::ModelUnavailable(message),
            "bad_request" => Error::BadRequest(message),
            "schema_invalid" => Error::InvalidSchema(message),
            "guardrail_violation" => Error::GuardrailViolation(message),
            "context_exceeded" => Error::ContextExceeded(message),
            "unsupported_locale" | "concurrent_requests" | "generation_failed" => {
                Error::Generation(message)
            }
            _ => Error::Generation(message),
        }
    }

    /// Returns `true` when retrying the same request later might succeed.
    ///
    /// Useful for callers that want to back off while the on-device model
    /// finishes downloading rather than surfacing a hard failure.
    pub fn is_retryable(&self) -> bool {
        match self {
            Error::ModelUnavailable(message) => {
                message.contains("downloading") || message.contains("preparing")
            }
            Error::Timeout(_) => true,
            Error::Generation(message) => message.contains("already responding"),
            _ => false,
        }
    }
}

/// Convenient result alias.
pub type Result<T> = std::result::Result<T, Error>;
