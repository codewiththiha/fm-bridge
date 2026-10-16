use std::path::PathBuf;
use std::time::Duration;

/// Why Apple Intelligence can't serve requests on this Mac.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Unavailable {
    /// Intel or unsupported silicon. Permanent.
    DeviceNotEligible,
    /// Switched off in System Settings. The user can fix it.
    NotEnabled,
    /// Model assets are still downloading. Transient.
    ModelNotReady,
    /// macOS older than 26. Permanent until an upgrade.
    OsTooOld,
    /// A reason this crate doesn't know yet.
    Unknown,
}

impl Unavailable {
    pub(crate) fn from_token(token: &str) -> Self {
        match token {
            "device_not_eligible" => Self::DeviceNotEligible,
            "not_enabled" => Self::NotEnabled,
            "model_not_ready" => Self::ModelNotReady,
            "os_too_old" => Self::OsTooOld,
            _ => Self::Unknown,
        }
    }

    /// Whether waiting could fix it without the user.
    pub fn is_transient(self) -> bool {
        matches!(self, Self::ModelNotReady)
    }
}

impl std::fmt::Display for Unavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::DeviceNotEligible => "device not eligible",
            Self::NotEnabled => "Apple Intelligence not enabled",
            Self::ModelNotReady => "model not ready",
            Self::OsTooOld => "macOS 26 or newer required",
            Self::Unknown => "unavailable",
        })
    }
}

/// Everything a request can fail with.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// No helper binary at this path.
    #[error("FMBridge helper not found at {}", .0.display())]
    BinaryNotFound(PathBuf),

    /// The helper exists but couldn't be started.
    #[error("failed to start FMBridge at {}", path.display())]
    Spawn {
        /// The binary that failed to start.
        path: PathBuf,
        /// The OS error.
        #[source]
        source: std::io::Error,
    },

    /// Reading from or writing to the helper failed.
    #[error("i/o error talking to FMBridge")]
    Io(#[from] std::io::Error),

    /// A payload didn't encode or decode.
    #[error("bad JSON payload")]
    Json(#[from] serde_json::Error),

    /// Apple Intelligence can't run here right now.
    #[error("Apple Intelligence is unavailable: {message}")]
    ModelUnavailable {
        /// Stable cause; branch on this, not on `message`.
        reason: Unavailable,
        /// Human-readable detail.
        message: String,
    },

    /// The request was rejected before generation.
    #[error("invalid request: {0}")]
    BadRequest(String),

    /// The schema can't be built.
    #[error("invalid schema: {0}")]
    InvalidSchema(String),

    /// Safety guardrails blocked the prompt or the answer.
    #[error("blocked by safety guardrails: {0}")]
    GuardrailViolation(String),

    /// The model declined to answer.
    #[error("the model refused: {0}")]
    Refused(String),

    /// The model can't work in the prompt's language.
    #[error("unsupported language: {0}")]
    UnsupportedLanguage(String),

    /// Prompt plus answer outgrew the context window.
    #[error("context window exceeded: {0}")]
    ContextExceeded(String),

    /// The model is serving other work or is rate limited.
    #[error("model busy: {0}")]
    Busy(String),

    /// Any other model-side failure.
    #[error("generation failed: {0}")]
    Generation(String),

    /// The helper said something this crate can't read.
    #[error("protocol error: {0}")]
    Protocol(String),

    /// The helper exited before finishing.
    #[error("FMBridge exited unexpectedly ({status}){}", stderr_suffix(.stderr))]
    ProcessFailed {
        /// How the process ended.
        status: String,
        /// What it wrote to stderr.
        stderr: String,
    },

    /// Queueing plus generation took longer than the bridge timeout.
    #[error("timed out after {0:?}")]
    Timeout(Duration),
}

fn stderr_suffix(stderr: &str) -> String {
    match stderr.trim() {
        "" => String::new(),
        text => format!(": {text}"),
    }
}

impl Error {
    pub(crate) fn from_code(code: &str, message: String, reason: Option<&str>) -> Self {
        match code {
            "model_unavailable" => Self::ModelUnavailable {
                reason: reason.map_or(Unavailable::Unknown, Unavailable::from_token),
                message,
            },
            "bad_request" => Self::BadRequest(message),
            "schema_invalid" => Self::InvalidSchema(message),
            "guardrail_violation" => Self::GuardrailViolation(message),
            "refusal" => Self::Refused(message),
            "unsupported_locale" => Self::UnsupportedLanguage(message),
            "context_exceeded" => Self::ContextExceeded(message),
            "concurrent_requests" | "rate_limited" => Self::Busy(message),
            _ => Self::Generation(message),
        }
    }

    /// Whether the same request may succeed later.
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::ModelUnavailable { reason, .. } => reason.is_transient(),
            Self::Timeout(_) | Self::Busy(_) => true,
            _ => false,
        }
    }
}

/// The crate's result type.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_map_to_variants() {
        let cases = [
            ("refusal", "Refused"),
            ("unsupported_locale", "UnsupportedLanguage"),
            ("concurrent_requests", "Busy"),
            ("rate_limited", "Busy"),
            ("something_new", "Generation"),
        ];
        for (code, variant) in cases {
            let error = Error::from_code(code, "m".into(), None);
            assert!(format!("{error:?}").starts_with(variant), "{code}");
        }
    }

    #[test]
    fn only_transient_causes_retry() {
        let busy = Error::from_code("rate_limited", "m".into(), None);
        let ready = Error::from_code("model_unavailable", "m".into(), Some("model_not_ready"));
        let off = Error::from_code("model_unavailable", "m".into(), Some("not_enabled"));
        let refused = Error::from_code("refusal", "m".into(), None);
        assert!(busy.is_retryable() && ready.is_retryable());
        assert!(!off.is_retryable() && !refused.is_retryable());
    }

    #[test]
    fn unknown_reasons_degrade() {
        let error = Error::from_code("model_unavailable", "m".into(), Some("new_reason"));
        assert!(matches!(
            error,
            Error::ModelUnavailable {
                reason: Unavailable::Unknown,
                ..
            }
        ));
    }
}
