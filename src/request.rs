use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::schema::Schema;

/// Who wrote a [`Message`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Instructions; becomes Apple's `Instructions`.
    System,
    /// The person asking.
    User,
    /// An earlier model reply.
    Assistant,
}

/// One conversation turn.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    /// Who wrote it.
    pub role: Role,
    /// The text.
    pub content: String,
}

impl Message {
    /// A message from `role`.
    pub fn new(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
        }
    }

    /// A system message.
    pub fn system(content: impl Into<String>) -> Self {
        Self::new(Role::System, content)
    }

    /// A user message.
    pub fn user(content: impl Into<String>) -> Self {
        Self::new(Role::User, content)
    }

    /// An assistant message.
    pub fn assistant(content: impl Into<String>) -> Self {
        Self::new(Role::Assistant, content)
    }
}

/// How each token is picked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sampling {
    /// Always the likeliest token; deterministic.
    Greedy,
    /// One of the `top` likeliest tokens, seeded for repeatability.
    TopK {
        /// Candidate count.
        top: usize,
        /// Seed for reproducible output.
        seed: Option<u64>,
    },
}

/// A generation request, built by chaining.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Request {
    /// The conversation so far.
    pub messages: Vec<Message>,
    /// Sampling temperature; higher varies more.
    pub temperature: Option<f64>,
    /// Cap on answer length.
    pub max_tokens: Option<u32>,
    /// Token selection.
    pub sampling: Option<Sampling>,
    /// Forces structured output in this shape.
    pub schema: Option<Schema>,
    /// Streams partial objects for a structured request.
    pub stream_structured: bool,
}

impl Request {
    /// An empty request.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a message.
    pub fn message(mut self, role: Role, content: impl Into<String>) -> Self {
        self.messages.push(Message::new(role, content));
        self
    }

    /// Appends a system message.
    pub fn system(self, content: impl Into<String>) -> Self {
        self.message(Role::System, content)
    }

    /// Appends a user message.
    pub fn user(self, content: impl Into<String>) -> Self {
        self.message(Role::User, content)
    }

    /// Appends an assistant message.
    pub fn assistant(self, content: impl Into<String>) -> Self {
        self.message(Role::Assistant, content)
    }

    /// Appends several messages.
    pub fn messages<I: IntoIterator<Item = Message>>(mut self, messages: I) -> Self {
        self.messages.extend(messages);
        self
    }

    /// Sets the temperature.
    pub fn temperature(mut self, temperature: f64) -> Self {
        self.temperature = Some(temperature);
        self
    }

    /// Caps the answer length in tokens.
    pub fn max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }

    /// Sets token selection.
    pub fn sampling(mut self, sampling: Sampling) -> Self {
        self.sampling = Some(sampling);
        self
    }

    /// Forces the answer into `schema`.
    pub fn schema(mut self, schema: Schema) -> Self {
        self.schema = Some(schema);
        self
    }

    /// Streams partial objects; needs a schema and `Bridge::stream`.
    pub fn stream_structured(mut self, enabled: bool) -> Self {
        self.stream_structured = enabled;
        self
    }

    /// Rough prompt size, at four characters a token.
    pub fn estimated_tokens(&self) -> u32 {
        let chars: usize = self
            .messages
            .iter()
            .map(|m| m.content.chars().count())
            .sum();
        u32::try_from(chars / 4).unwrap_or(u32::MAX)
    }

    pub(crate) fn validate(&self) -> Result<()> {
        let spoken = |m: &&Message| m.role != Role::System && !m.content.trim().is_empty();
        if !self.messages.iter().any(|m| spoken(&m)) {
            return Err(Error::BadRequest(
                "needs a non-empty user or assistant message".into(),
            ));
        }
        if let Some(t) = self.temperature
            && !(t.is_finite() && t >= 0.0)
        {
            return Err(Error::BadRequest(
                "temperature must be finite and >= 0".into(),
            ));
        }
        if self.max_tokens == Some(0) {
            return Err(Error::BadRequest("max_tokens must be at least 1".into()));
        }
        if let Some(Sampling::TopK { top: 0, .. }) = self.sampling {
            return Err(Error::BadRequest(
                "top-k needs at least one candidate".into(),
            ));
        }
        if let Some(schema) = &self.schema {
            schema.validate().map_err(Error::InvalidSchema)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_requests_without_a_spoken_turn() {
        for request in [
            Request::new().system("only instructions"),
            Request::new().system("x").user("   "),
        ] {
            assert!(matches!(request.validate(), Err(Error::BadRequest(_))));
        }
    }

    #[test]
    fn rejects_bad_numbers() {
        let base = || Request::new().user("hi");
        let bad = [
            base().temperature(f64::NAN),
            base().temperature(-1.0),
            base().max_tokens(0),
            base().sampling(Sampling::TopK { top: 0, seed: None }),
        ];
        for request in bad {
            assert!(matches!(request.validate(), Err(Error::BadRequest(_))));
        }
    }

    #[test]
    fn estimates_tokens_by_characters() {
        let request = Request::new().system("abcd").user("efgh");
        assert_eq!(request.estimated_tokens(), 2);
    }
}
