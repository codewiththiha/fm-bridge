//! Error types for the crate.

use std::path::PathBuf;

/// Why Apple Intelligence is not usable on this machine.
///
/// The helper reports this as a stable machine-readable token alongside the
/// human-readable message, so callers can branch on the *cause* without
/// pattern-matching on prose that may be reworded at any time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Unavailable {
    /// The hardware cannot run Apple Intelligence (Intel, or a Mac whose
    /// silicon is not supported). Nothing the user does will fix this.
    DeviceNotEligible,
    /// The device is capable but Apple Intelligence is switched off in
    /// System Settings. The user can fix this.
    NotEnabled,
    /// Model assets are still downloading or being prepared. This is
    /// transient — the same request may succeed shortly.
    ModelNotReady,
    /// The host is older than macOS 26, so `FoundationModels` is absent.
    OsTooOld,
    /// The helper reported a reason this version of the crate does not know,
    /// most likely one introduced by a newer SDK.
    Unknown,
}

impl Unavailable {
    /// Maps the helper's wire token to a reason.
    pub(crate) fn from_token(token: &str) -> Self {
        match token {
            "device_not_eligible" => Unavailable::DeviceNotEligible,
            "not_enabled" => Unavailable::NotEnabled,
            "model_not_ready" => Unavailable::ModelNotReady,
            "os_too_old" => Unavailable::OsTooOld,
            _ => Unavailable::Unknown,
        }
    }

    /// Whether waiting and trying again could plausibly succeed.
    ///
    /// Only [`ModelNotReady`](Self::ModelNotReady) is transient; an ineligible
    /// device, a disabled setting, or an old OS all need human intervention.
    pub fn is_transient(self) -> bool {
        matches!(self, Unavailable::ModelNotReady)
    }
}

impl std::fmt::Display for Unavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Unavailable::DeviceNotEligible => "device not eligible",
            Unavailable::NotEnabled => "Apple Intelligence not enabled",
            Unavailable::ModelNotReady => "model not ready",
            Unavailable::OsTooOld => "macOS 26 or newer required",
            Unavailable::Unknown => "unavailable",
        };
        f.write_str(text)
    }
}

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
    /// switched off, or the model may still be downloading. Match on
    /// [`reason`](Self::ModelUnavailable::reason) rather than the message when
    /// deciding how to react — see [`Unavailable`].
    #[error("Apple Intelligence is unavailable: {message}")]
    ModelUnavailable {
        /// Machine-readable cause, safe to branch on.
        reason: Unavailable,
        /// Human-readable explanation from the helper, for display and logs.
        message: String,
    },

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
    /// `reason` is the helper's optional unavailability token, only meaningful
    /// for `model_unavailable`.
    pub(crate) fn from_code(code: &str, message: String, reason: Option<&str>) -> Self {
        match code {
            "model_unavailable" => Error::ModelUnavailable {
                reason: reason.map_or(Unavailable::Unknown, Unavailable::from_token),
                message,
            },
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
            // Only a model that is still being prepared is worth waiting for;
            // an ineligible device or a disabled setting never resolves itself.
            Error::ModelUnavailable { reason, .. } => reason.is_transient(),
            Error::Timeout(_) => true,
            Error::Generation(message) => message.contains("already responding"),
            _ => false,
        }
    }
}

/// Convenient result alias.
pub type Result<T> = std::result::Result<T, Error>;
