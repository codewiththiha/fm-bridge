//! The NDJSON protocol; `swift/Sources/FMBridge/Protocol.swift` mirrors it.

use serde::Serialize;
use serde_json::Value;

use crate::error::{Error, Result};
use crate::request::{Message, Request, Sampling};
use crate::response::{Availability, StreamEvent, Usage};
use crate::schema::Schema;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WireRequest<'a> {
    messages: &'a [Message],
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_k: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    seed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    greedy: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    schema: Option<&'a Schema>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    stream_structured: bool,
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

/// One line the helper wrote.
#[derive(Debug, PartialEq)]
pub(crate) enum Line {
    Event(StreamEvent),
    Ready(Availability),
    /// Blank, not JSON, or an unknown key: stray output never breaks a reply.
    Noise,
}

pub(crate) fn parse_line(line: &str) -> Result<Line> {
    let Ok(Value::Object(object)) = serde_json::from_str::<Value>(line.trim()) else {
        return Ok(Line::Noise);
    };
    if let Some(message) = object.get("error").and_then(Value::as_str) {
        let code = object.get("code").and_then(Value::as_str);
        let reason = object.get("reason").and_then(Value::as_str);
        let code = code.unwrap_or("generation_failed");
        return Err(Error::from_code(code, message.to_owned(), reason));
    }
    if object.get("done").and_then(Value::as_bool) == Some(true) {
        let usage = object.get("usage").map_or_else(Usage::default, |u| Usage {
            prompt_tokens: field_u32(u, "promptTokens").unwrap_or(0),
            completion_tokens: field_u32(u, "completionTokens").unwrap_or(0),
        });
        return Ok(Line::Event(StreamEvent::Done(usage)));
    }
    if let Some(value) = object.get("structured") {
        return Ok(Line::Event(StreamEvent::Structured(value.clone())));
    }
    if let Some(value) = object.get("snapshot") {
        return Ok(Line::Event(StreamEvent::Snapshot(value.clone())));
    }
    if let Some(delta) = object.get("delta").and_then(Value::as_str) {
        return Ok(Line::Event(StreamEvent::Delta(delta.to_owned())));
    }
    if let Some(ready) = object.get("ready") {
        let languages = ready
            .get("supportedLanguages")
            .and_then(Value::as_array)
            .map(|l| {
                l.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        return Ok(Line::Ready(Availability {
            context_size: field_u32(ready, "contextSize"),
            languages,
        }));
    }
    Ok(Line::Noise)
}

fn field_u32(value: &Value, key: &str) -> Option<u32> {
    let n = value.get(key).and_then(Value::as_u64)?;
    Some(u32::try_from(n).unwrap_or(u32::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::SchemaProperty;

    #[test]
    fn serializes_camel_case_keys() {
        let request = Request::new()
            .system("be brief")
            .user("hello")
            .temperature(0.5)
            .max_tokens(128)
            .sampling(Sampling::TopK {
                top: 40,
                seed: Some(7),
            });
        let json = serde_json::to_value(WireRequest::new(&request, true)).unwrap();
        assert_eq!(json["stream"], true);
        assert_eq!(json["maxTokens"], 128);
        assert_eq!(json["topK"], 40);
        assert_eq!(json["seed"], 7);
        assert_eq!(json["messages"][1]["content"], "hello");
        assert!(json.get("greedy").is_none());
    }

    #[test]
    fn structured_streaming_needs_a_stream() {
        let schema = Schema::new("S", vec![SchemaProperty::string("a").optional()]);
        let request = Request::new()
            .user("hi")
            .schema(schema)
            .stream_structured(true);
        let buffered = serde_json::to_value(WireRequest::new(&request, false)).unwrap();
        let streamed = serde_json::to_value(WireRequest::new(&request, true)).unwrap();
        assert!(buffered.get("streamStructured").is_none());
        assert_eq!(streamed["streamStructured"], true);
        assert_eq!(streamed["schema"]["properties"][0]["optional"], true);
    }

    #[test]
    fn parses_every_line_kind() {
        let done = r#"{"done":true,"usage":{"promptTokens":3,"completionTokens":99999999999}}"#;
        let ready = r#"{"ready":{"contextSize":4096,"supportedLanguages":["en-US"]}}"#;
        assert_eq!(
            parse_line(done).unwrap(),
            Line::Event(StreamEvent::Done(Usage {
                prompt_tokens: 3,
                completion_tokens: u32::MAX
            }))
        );
        let Line::Ready(model) = parse_line(ready).unwrap() else {
            panic!("not ready")
        };
        assert_eq!(model.context_size, Some(4096));
        for noise in ["", "garbage", "[1]", r#"{"unknown":1}"#] {
            assert_eq!(parse_line(noise).unwrap(), Line::Noise);
        }
        let error = parse_line(r#"{"error":"no","code":"refusal"}"#).unwrap_err();
        assert!(matches!(error, Error::Refused(_)));
    }
}
