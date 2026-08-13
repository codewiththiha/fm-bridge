//! Request, response, and schema types.

use serde::{Deserialize, Serialize};

/// Who authored a [`Message`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// Behavioural instructions. Mapped onto Apple's `Instructions`.
    System,
    /// Input from the end user.
    User,
    /// A previous reply from the model.
    Assistant,
}

/// A single turn in the conversation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    /// Who authored the message.
    pub role: Role,
    /// The message text.
    pub content: String,
}

impl Message {
    /// Creates a message.
    pub fn new(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
        }
    }

    /// Creates a `system` message.
    pub fn system(content: impl Into<String>) -> Self {
        Self::new(Role::System, content)
    }

    /// Creates a `user` message.
    pub fn user(content: impl Into<String>) -> Self {
        Self::new(Role::User, content)
    }

    /// Creates an `assistant` message.
    pub fn assistant(content: impl Into<String>) -> Self {
        Self::new(Role::Assistant, content)
    }
}

/// The JSON type of a [`SchemaProperty`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SchemaType {
    /// A UTF-8 string.
    String,
    /// A signed integer.
    Integer,
    /// A double-precision float.
    Number,
    /// `true` or `false`.
    Boolean,
    /// A nested object; set [`SchemaProperty::properties`].
    Object,
    /// A homogeneous list; set [`SchemaProperty::items`].
    Array,
}

/// One field of a [`Schema`].
///
/// Build these with the constructors ([`SchemaProperty::string`],
/// [`SchemaProperty::integer`], …) and the chainable modifiers rather than
/// filling in every field by hand.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SchemaProperty {
    /// Field name as it will appear in the generated JSON.
    pub name: String,
    /// Natural-language hint that steers the model. Strongly recommended.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The field's type.
    #[serde(rename = "type")]
    pub property_type: SchemaType,
    /// Nested fields, for [`SchemaType::Object`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<Vec<SchemaProperty>>,
    /// Element schema, for [`SchemaType::Array`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<Box<SchemaProperty>>,
    /// Inclusive `[min, max]` bounds for numeric types.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<[f64; 2]>,
    /// Restricts a string to a fixed set of values.
    #[serde(rename = "anyOf", skip_serializing_if = "Option::is_none")]
    pub any_of: Option<Vec<String>>,
    /// Regex the generated string must match.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    /// `[min, max]` element counts for arrays; either may be `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<[Option<usize>; 2]>,
    /// Whether the model may omit this field. Defaults to `false`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub optional: bool,
}

impl SchemaProperty {
    fn bare(name: impl Into<String>, property_type: SchemaType) -> Self {
        Self {
            name: name.into(),
            description: None,
            property_type,
            properties: None,
            items: None,
            range: None,
            any_of: None,
            pattern: None,
            count: None,
            optional: false,
        }
    }

    /// A string field.
    pub fn string(name: impl Into<String>) -> Self {
        Self::bare(name, SchemaType::String)
    }

    /// An integer field.
    pub fn integer(name: impl Into<String>) -> Self {
        Self::bare(name, SchemaType::Integer)
    }

    /// A floating-point field.
    pub fn number(name: impl Into<String>) -> Self {
        Self::bare(name, SchemaType::Number)
    }

    /// A boolean field.
    pub fn boolean(name: impl Into<String>) -> Self {
        Self::bare(name, SchemaType::Boolean)
    }

    /// A nested object field.
    pub fn object(name: impl Into<String>, properties: Vec<SchemaProperty>) -> Self {
        Self {
            properties: Some(properties),
            ..Self::bare(name, SchemaType::Object)
        }
    }

    /// An array field whose elements follow `items`.
    pub fn array(name: impl Into<String>, items: SchemaProperty) -> Self {
        Self {
            items: Some(Box::new(items)),
            ..Self::bare(name, SchemaType::Array)
        }
    }

    /// Adds a natural-language description. This meaningfully improves output.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Constrains a numeric field to an inclusive range.
    pub fn range(mut self, min: f64, max: f64) -> Self {
        self.range = Some([min, max]);
        self
    }

    /// Constrains a string field to one of `choices`.
    pub fn any_of<I, S>(mut self, choices: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.any_of = Some(choices.into_iter().map(Into::into).collect());
        self
    }

    /// Constrains a string field to a regular expression.
    pub fn pattern(mut self, pattern: impl Into<String>) -> Self {
        self.pattern = Some(pattern.into());
        self
    }

    /// Constrains how many elements an array field may hold.
    pub fn count(mut self, min: impl Into<Option<usize>>, max: impl Into<Option<usize>>) -> Self {
        self.count = Some([min.into(), max.into()]);
        self
    }

    /// Marks the field optional, letting the model omit it.
    pub fn optional(mut self) -> Self {
        self.optional = true;
        self
    }

    /// Validates the property tree, returning a human-readable reason on failure.
    pub(crate) fn validate(&self) -> std::result::Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("a schema property has an empty name".into());
        }
        match self.property_type {
            SchemaType::Object => {
                let nested = self
                    .properties
                    .as_ref()
                    .filter(|p| !p.is_empty())
                    .ok_or_else(|| {
                        format!("object property '{}' has no nested properties", self.name)
                    })?;
                for child in nested {
                    child.validate()?;
                }
            }
            SchemaType::Array => {
                let items = self
                    .items
                    .as_ref()
                    .ok_or_else(|| format!("array property '{}' is missing `items`", self.name))?;
                items.validate()?;
                if let Some([Some(min), Some(max)]) = self.count {
                    if min > max {
                        return Err(format!(
                            "array property '{}' has min count > max count",
                            self.name
                        ));
                    }
                }
            }
            SchemaType::Integer | SchemaType::Number => {
                if let Some([min, max]) = self.range {
                    if min > max {
                        return Err(format!("property '{}' has an inverted range", self.name));
                    }
                }
            }
            SchemaType::String | SchemaType::Boolean => {}
        }
        Ok(())
    }
}

/// A runtime-defined JSON schema the model is forced to satisfy.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Schema {
    /// Name of the generated object type.
    pub name: String,
    /// Optional description of the object as a whole.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The object's fields.
    pub properties: Vec<SchemaProperty>,
}

impl Schema {
    /// Creates a schema from a name and its fields.
    pub fn new(name: impl Into<String>, properties: Vec<SchemaProperty>) -> Self {
        Self {
            name: name.into(),
            description: None,
            properties,
        }
    }

    /// Adds a description of the object as a whole.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Validates the schema locally, before spawning a process.
    pub(crate) fn validate(&self) -> std::result::Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("schema name must not be empty".into());
        }
        if self.properties.is_empty() {
            return Err(format!("schema '{}' declares no properties", self.name));
        }
        for property in &self.properties {
            property.validate()?;
        }
        Ok(())
    }
}

/// How the model picks each token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sampling {
    /// Always take the most likely token. Deterministic.
    Greedy,
    /// Sample from the `top` most likely tokens, optionally with a fixed seed.
    TopK {
        /// How many candidate tokens to consider.
        top: usize,
        /// Seed for reproducible output.
        seed: Option<u64>,
    },
}

/// A generation request.
///
/// Build one with the chainable methods:
///
/// ```
/// use fm_bridge::Request;
///
/// let request = Request::new()
///     .system("You are terse.")
///     .user("Say hi.")
///     .temperature(0.4)
///     .max_tokens(64);
/// assert_eq!(request.messages.len(), 2);
/// ```
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Request {
    /// The conversation so far.
    pub messages: Vec<Message>,
    /// Sampling temperature. Higher is more varied.
    pub temperature: Option<f64>,
    /// Hard cap on response length.
    pub max_tokens: Option<u32>,
    /// Token-selection strategy.
    pub sampling: Option<Sampling>,
    /// When set, forces structured output matching this schema.
    pub schema: Option<Schema>,
    /// Emit partial snapshots while a structured response is generated.
    pub stream_structured: bool,
}

impl Request {
    /// Creates an empty request.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a message.
    pub fn message(mut self, role: Role, content: impl Into<String>) -> Self {
        self.messages.push(Message::new(role, content));
        self
    }

    /// Appends a `system` message.
    pub fn system(self, content: impl Into<String>) -> Self {
        self.message(Role::System, content)
    }

    /// Appends a `user` message.
    pub fn user(self, content: impl Into<String>) -> Self {
        self.message(Role::User, content)
    }

    /// Appends an `assistant` message.
    pub fn assistant(self, content: impl Into<String>) -> Self {
        self.message(Role::Assistant, content)
    }

    /// Appends many messages at once.
    pub fn messages<I: IntoIterator<Item = Message>>(mut self, messages: I) -> Self {
        self.messages.extend(messages);
        self
    }

    /// Sets the sampling temperature.
    pub fn temperature(mut self, temperature: f64) -> Self {
        self.temperature = Some(temperature);
        self
    }

    /// Caps the number of tokens in the response.
    pub fn max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = Some(max_tokens);
        self
    }

    /// Sets the token-selection strategy.
    pub fn sampling(mut self, sampling: Sampling) -> Self {
        self.sampling = Some(sampling);
        self
    }

    /// Forces the response to satisfy `schema`.
    pub fn schema(mut self, schema: Schema) -> Self {
        self.schema = Some(schema);
        self
    }

    /// Requests partial snapshots while a structured response streams.
    ///
    /// Only meaningful together with [`schema`](Self::schema) and
    /// [`Bridge::stream`](crate::Bridge::stream).
    pub fn stream_structured(mut self, enabled: bool) -> Self {
        self.stream_structured = enabled;
        self
    }

    pub(crate) fn validate(&self) -> crate::Result<()> {
        if !self.messages.iter().any(|m| m.role != Role::System) {
            return Err(crate::Error::BadRequest(
                "request needs at least one user or assistant message".into(),
            ));
        }
        if self.messages.iter().all(|m| m.content.trim().is_empty()) {
            return Err(crate::Error::BadRequest("all messages are empty".into()));
        }
        if let Some(temperature) = self.temperature {
            if !temperature.is_finite() || temperature < 0.0 {
                return Err(crate::Error::BadRequest(
                    "temperature must be a finite, non-negative number".into(),
                ));
            }
        }
        if let Some(schema) = &self.schema {
            schema.validate().map_err(crate::Error::InvalidSchema)?;
        }
        Ok(())
    }
}

/// Approximate token accounting.
///
/// The Foundation Models framework does not expose real token counts to
/// third-party callers, so these are **estimates** derived from character
/// length (roughly four characters per token). Use them for rough budgeting,
/// not for billing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    /// Estimated tokens consumed by instructions and prompt.
    pub prompt_tokens: u32,
    /// Estimated tokens produced in the response.
    pub completion_tokens: u32,
}

impl Usage {
    /// Estimated total tokens.
    pub fn total_tokens(&self) -> u32 {
        self.prompt_tokens.saturating_add(self.completion_tokens)
    }
}

/// A finished, non-streaming response.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Completion {
    /// Concatenated text. Empty for structured responses.
    pub text: String,
    /// The generated object, when the request carried a schema.
    pub structured: Option<serde_json::Value>,
    /// Approximate token usage.
    pub usage: Usage,
}

impl Completion {
    /// Deserializes the structured payload into `T`.
    ///
    /// Returns [`Error::Protocol`](crate::Error::Protocol) when the response
    /// carried no structured payload.
    pub fn parse<T: serde::de::DeserializeOwned>(&self) -> crate::Result<T> {
        let value = self.structured.as_ref().ok_or_else(|| {
            crate::Error::Protocol("response contained no structured payload".into())
        })?;
        Ok(serde_json::from_value(value.clone())?)
    }
}

/// One event from a streaming response.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum StreamEvent {
    /// A newly generated chunk of text. Concatenate these in order.
    Delta(String),
    /// A partially-filled structured object.
    ///
    /// Only emitted when [`Request::stream_structured`] is enabled.
    Snapshot(serde_json::Value),
    /// The finished structured object.
    Structured(serde_json::Value),
    /// Generation finished; carries the usage estimate.
    Done(Usage),
}

// ── Wire types ───────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub(crate) struct WireRequest<'a> {
    pub messages: &'a [Message],
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,
    #[serde(rename = "maxTokens", skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(rename = "topK", skip_serializing_if = "Option::is_none")]
    pub top_k: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub greedy: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<&'a Schema>,
    #[serde(
        rename = "streamStructured",
        skip_serializing_if = "std::ops::Not::not"
    )]
    pub stream_structured: bool,
}

impl<'a> WireRequest<'a> {
    pub(crate) fn new(request: &'a Request, stream: bool) -> Self {
        let (top_k, seed, greedy) = match request.sampling {
            Some(Sampling::Greedy) => (None, None, Some(true)),
            Some(Sampling::TopK { top, seed }) => (Some(top), seed, None),
            None => (None, None, None),
        };
        Self {
            messages: &request.messages,
            stream,
            temperature: request.temperature,
            max_tokens: request.max_tokens,
            top_k,
            seed,
            greedy,
            schema: request.schema.as_ref(),
            stream_structured: request.stream_structured && stream,
        }
    }
}
