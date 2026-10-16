use serde::de::DeserializeOwned;

use crate::error::{Error, Result};

/// Estimated token counts; Apple exposes no tokenizer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    /// Instructions and prompt.
    pub prompt_tokens: u32,
    /// The answer.
    pub completion_tokens: u32,
}

impl Usage {
    /// Both counts together.
    pub fn total_tokens(&self) -> u32 {
        self.prompt_tokens.saturating_add(self.completion_tokens)
    }
}

/// A finished answer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Completion {
    /// The text; empty for structured answers.
    pub text: String,
    /// The object, for a request with a schema.
    pub structured: Option<serde_json::Value>,
    /// Estimated usage.
    pub usage: Usage,
}

impl Completion {
    /// Decodes the structured answer into `T`.
    pub fn parse<T: DeserializeOwned>(&self) -> Result<T> {
        let value = self
            .structured
            .as_ref()
            .ok_or_else(|| Error::Protocol("no structured payload".into()))?;
        Ok(T::deserialize(value)?)
    }
}

/// One event of a streamed answer.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum StreamEvent {
    /// New text; concatenate in order.
    Delta(String),
    /// A partly filled object; unfilled fields are absent.
    Snapshot(serde_json::Value),
    /// The finished object.
    Structured(serde_json::Value),
    /// The end, with estimated usage.
    Done(Usage),
}

/// What the on-device model can do, from a probe.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Availability {
    /// Context window in tokens, when the OS reports it.
    pub context_size: Option<u32>,
    /// BCP-47 identifiers the model supports, sorted.
    pub languages: Vec<String>,
}

impl Availability {
    /// Whether `tag` (like `en` or `es-MX`) shares a language with the model.
    pub fn supports(&self, tag: &str) -> bool {
        let wanted = primary(tag);
        !wanted.is_empty() && self.languages.iter().any(|l| primary(l) == wanted)
    }
}

fn primary(tag: &str) -> String {
    tag.split(['-', '_'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_languages_by_primary_subtag() {
        let model = Availability {
            context_size: Some(4096),
            languages: vec!["en-Latn-US".into(), "es-Latn-ES".into()],
        };
        assert!(model.supports("en") && model.supports("ES_mx"));
        assert!(!model.supports("my") && !model.supports(""));
    }

    #[test]
    fn parse_needs_a_payload() {
        let empty = Completion::default();
        assert!(matches!(empty.parse::<u8>(), Err(Error::Protocol(_))));
    }
}
