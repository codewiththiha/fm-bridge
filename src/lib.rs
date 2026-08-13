//! Idiomatic async Rust bindings for Apple's on-device **Foundation Models**
//! framework.
//!
//! Apple's `FoundationModels` framework is Swift-only and has no C ABI, so this
//! crate talks to a small Swift helper binary (`FMBridge`) over stdin and
//! stdout using newline-delimited JSON. One process is spawned per request and
//! is torn down when the request finishes — or immediately, if you drop the
//! future or stream.
//!
//! # Requirements
//!
//! * Apple Silicon Mac running macOS 26 or later
//! * Apple Intelligence enabled in System Settings
//! * The `FMBridge` helper built from the `swift/` directory of this
//!   repository:
//!
//! ```text
//! swift build -c release --package-path swift
//! export FM_BRIDGE_BIN="$PWD/swift/.build/release/FMBridge"
//! ```
//!
//! # Text generation
//!
//! ```no_run
//! use fm_bridge::{Bridge, Request};
//!
//! # async fn run() -> fm_bridge::Result<()> {
//! let bridge = Bridge::from_env()?;
//! let response = bridge
//!     .complete(
//!         Request::new()
//!             .system("You are a concise assistant.")
//!             .user("Name three seabirds."),
//!     )
//!     .await?;
//!
//! println!("{}", response.text);
//! # Ok(())
//! # }
//! ```
//!
//! # Streaming
//!
//! ```no_run
//! use fm_bridge::{Bridge, Request, StreamEvent};
//! use futures::StreamExt;
//!
//! # async fn run() -> fm_bridge::Result<()> {
//! let bridge = Bridge::from_env()?;
//! let mut stream = Box::pin(bridge.stream(Request::new().user("Write a haiku.")));
//!
//! while let Some(event) = stream.next().await {
//!     match event? {
//!         StreamEvent::Delta(text) => print!("{text}"),
//!         StreamEvent::Done(usage) => println!("\n~{} tokens", usage.total_tokens()),
//!         _ => {}
//!     }
//! }
//! # Ok(())
//! # }
//! ```
//!
//! # Structured output
//!
//! Supplying a [`Schema`] switches the model into constrained decoding, so the
//! response is guaranteed to be well-formed JSON matching your shape.
//!
//! ```no_run
//! use fm_bridge::{Bridge, Request, Schema, SchemaProperty};
//! use serde::Deserialize;
//!
//! #[derive(Deserialize)]
//! struct Recipe {
//!     title: String,
//!     minutes: i64,
//!     ingredients: Vec<String>,
//! }
//!
//! # async fn run() -> fm_bridge::Result<()> {
//! let schema = Schema::new(
//!     "Recipe",
//!     vec![
//!         SchemaProperty::string("title").description("Name of the dish"),
//!         SchemaProperty::integer("minutes").description("Cook time").range(1.0, 240.0),
//!         SchemaProperty::array("ingredients", SchemaProperty::string("item")).count(2, 10),
//!     ],
//! );
//!
//! let bridge = Bridge::from_env()?;
//! let response = bridge
//!     .complete(Request::new().user("Invent a quick pasta dish.").schema(schema))
//!     .await?;
//!
//! let recipe: Recipe = response.parse()?;
//! println!("{} takes {} minutes", recipe.title, recipe.minutes);
//! # Ok(())
//! # }
//! ```
//!
//! # Concurrency
//!
//! A bridge runs **one request at a time** by default
//! ([`DEFAULT_MAX_CONCURRENCY`]). The on-device model is a single shared
//! resource, so parallelism is opt-in rather than accidental:
//!
//! ```no_run
//! use fm_bridge::{Bridge, Request};
//!
//! # async fn run() -> fm_bridge::Result<()> {
//! let bridge = Bridge::from_env()?.max_concurrency(4);
//!
//! let tasks: Vec<_> = ["one", "two", "three"]
//!     .into_iter()
//!     .map(|prompt| {
//!         // Clones share the same 4-slot budget.
//!         let bridge = bridge.clone();
//!         tokio::spawn(async move { bridge.complete(Request::new().user(prompt)).await })
//!     })
//!     .collect();
//!
//! for task in tasks {
//!     println!("{}", task.await.expect("task")?.text);
//! }
//! # Ok(())
//! # }
//! ```
//!
//! Requests past the limit **queue** instead of failing, and each caller always
//! receives the response to its own request — every request gets a private
//! helper process, so replies cannot be crossed. Queue time counts toward
//! [`Bridge::timeout`], so a saturated bridge eventually reports
//! [`Error::Timeout`] rather than blocking forever, and dropping a future or
//! stream returns its slot immediately.
//!
//! See `examples/concurrent.rs` for a runnable demonstration.
//!
//! # A note on token counts
//!
//! Apple does not expose a tokenizer or token counter to third-party code, so
//! every [`Usage`] figure this crate reports is an **estimate** based on
//! character length. Treat it as a rough signal only.

#![forbid(unsafe_code)]
#![warn(missing_docs, missing_debug_implementations, rust_2018_idioms)]
#![cfg_attr(docsrs, feature(doc_cfg))]

mod client;
mod error;
mod types;

pub use client::{BINARY_ENV_VAR, Bridge, DEFAULT_MAX_CONCURRENCY};
pub use error::{Error, Result};
pub use types::{
    Completion, Message, Request, Role, Sampling, Schema, SchemaProperty, SchemaType, StreamEvent,
    Usage,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_request_with_camel_case_keys() {
        let request = Request::new()
            .system("be brief")
            .user("hello")
            .temperature(0.5)
            .max_tokens(128)
            .sampling(Sampling::TopK {
                top: 40,
                seed: Some(7),
            });

        let wire = types::WireRequest::new(&request, true);
        let json = serde_json::to_value(&wire).unwrap();

        assert_eq!(json["stream"], true);
        assert_eq!(json["maxTokens"], 128);
        assert_eq!(json["topK"], 40);
        assert_eq!(json["seed"], 7);
        assert_eq!(json["messages"][0]["role"], "system");
        assert_eq!(json["messages"][1]["content"], "hello");
        assert!(json.get("greedy").is_none());
    }

    #[test]
    fn greedy_sampling_sets_flag() {
        let request = Request::new().user("hi").sampling(Sampling::Greedy);
        let json = serde_json::to_value(types::WireRequest::new(&request, false)).unwrap();

        assert_eq!(json["greedy"], true);
        assert_eq!(json["stream"], false);
        assert!(json.get("topK").is_none());
    }

    #[test]
    fn serializes_nested_schema() {
        let schema = Schema::new(
            "Person",
            vec![
                SchemaProperty::string("name"),
                SchemaProperty::string("mood").any_of(["happy", "sad"]),
                SchemaProperty::integer("age").range(0.0, 120.0).optional(),
                SchemaProperty::array("tags", SchemaProperty::string("tag")).count(1, 5),
                SchemaProperty::object("address", vec![SchemaProperty::string("city")]),
            ],
        );
        let request = Request::new().user("hi").schema(schema);
        let json = serde_json::to_value(types::WireRequest::new(&request, false)).unwrap();
        let properties = &json["schema"]["properties"];

        assert_eq!(properties[0]["type"], "string");
        assert_eq!(properties[1]["anyOf"][1], "sad");
        assert_eq!(properties[2]["optional"], true);
        assert_eq!(properties[2]["range"][1], 120.0);
        assert_eq!(properties[3]["items"]["type"], "string");
        assert_eq!(properties[3]["count"][1], 5);
        assert_eq!(properties[4]["properties"][0]["name"], "city");
        // `optional: false` is omitted rather than sent explicitly.
        assert!(properties[0].get("optional").is_none());
    }

    #[test]
    fn rejects_requests_without_a_user_turn() {
        let error = Request::new()
            .system("only instructions")
            .validate()
            .unwrap_err();
        assert!(matches!(error, Error::BadRequest(_)));
    }

    #[test]
    fn rejects_malformed_schemas() {
        let empty = Request::new()
            .user("hi")
            .schema(Schema::new("Empty", vec![]));
        assert!(matches!(empty.validate(), Err(Error::InvalidSchema(_))));

        let bad_object = Request::new().user("hi").schema(Schema::new(
            "S",
            vec![SchemaProperty::object("nested", vec![])],
        ));
        assert!(matches!(
            bad_object.validate(),
            Err(Error::InvalidSchema(_))
        ));

        let inverted = Request::new().user("hi").schema(Schema::new(
            "S",
            vec![SchemaProperty::integer("n").range(10.0, 1.0)],
        ));
        assert!(matches!(inverted.validate(), Err(Error::InvalidSchema(_))));
    }

    #[test]
    fn rejects_invalid_temperature() {
        let request = Request::new().user("hi").temperature(-1.0);
        assert!(matches!(request.validate(), Err(Error::BadRequest(_))));
    }

    #[test]
    fn parses_every_event_kind() {
        use client::parse_line;

        assert_eq!(
            parse_line(r#"{"delta":"hi"}"#).unwrap(),
            Some(StreamEvent::Delta("hi".into()))
        );
        assert!(matches!(
            parse_line(r#"{"structured":{"a":1}}"#).unwrap(),
            Some(StreamEvent::Structured(_))
        ));
        assert!(matches!(
            parse_line(r#"{"snapshot":{"a":null}}"#).unwrap(),
            Some(StreamEvent::Snapshot(_))
        ));

        let done =
            parse_line(r#"{"done":true,"usage":{"promptTokens":3,"completionTokens":9}}"#).unwrap();
        assert_eq!(
            done,
            Some(StreamEvent::Done(Usage {
                prompt_tokens: 3,
                completion_tokens: 9
            }))
        );

        // Unknown, blank, and non-JSON lines are ignored rather than fatal.
        assert_eq!(parse_line("").unwrap(), None);
        assert_eq!(parse_line("not json at all").unwrap(), None);
        assert_eq!(
            parse_line(r#"{"ready":{"model":"on-device"}}"#).unwrap(),
            None
        );
    }

    #[test]
    fn maps_error_codes_to_variants() {
        use client::parse_line;

        let cases = [
            (
                r#"{"error":"off","code":"model_unavailable"}"#,
                "model_unavailable",
            ),
            (r#"{"error":"bad","code":"bad_request"}"#, "bad_request"),
            (
                r#"{"error":"nope","code":"schema_invalid"}"#,
                "schema_invalid",
            ),
            (
                r#"{"error":"blocked","code":"guardrail_violation"}"#,
                "guardrail_violation",
            ),
            (
                r#"{"error":"full","code":"context_exceeded"}"#,
                "context_exceeded",
            ),
        ];

        for (line, code) in cases {
            let error = parse_line(line).unwrap_err();
            let matched = matches!(
                (code, &error),
                ("model_unavailable", Error::ModelUnavailable(_))
                    | ("bad_request", Error::BadRequest(_))
                    | ("schema_invalid", Error::InvalidSchema(_))
                    | ("guardrail_violation", Error::GuardrailViolation(_))
                    | ("context_exceeded", Error::ContextExceeded(_))
            );
            assert!(matched, "code {code} produced {error:?}");
        }

        // Unknown codes, and errors with no code at all, degrade gracefully.
        assert!(matches!(
            parse_line(r#"{"error":"???","code":"brand_new_code"}"#).unwrap_err(),
            Error::Generation(_)
        ));
        assert!(matches!(
            parse_line(r#"{"error":"plain"}"#).unwrap_err(),
            Error::Generation(_)
        ));
    }

    #[test]
    fn completion_parses_structured_payload() {
        #[derive(serde::Deserialize)]
        struct Person {
            name: String,
        }

        let completion = Completion {
            structured: Some(serde_json::json!({ "name": "Ada" })),
            ..Default::default()
        };
        assert_eq!(completion.parse::<Person>().unwrap().name, "Ada");

        let empty = Completion::default();
        assert!(matches!(empty.parse::<Person>(), Err(Error::Protocol(_))));
    }

    #[test]
    fn usage_total_saturates() {
        let usage = Usage {
            prompt_tokens: u32::MAX,
            completion_tokens: 10,
        };
        assert_eq!(usage.total_tokens(), u32::MAX);
    }

    /// Covers `from_env`'s parsing without mutating the process environment,
    /// which is `unsafe` in edition 2024 and racy under a threaded test runner.
    #[test]
    fn from_env_rejects_a_missing_or_empty_variable() {
        let missing = Bridge::from_env_value(None).unwrap_err();
        assert!(matches!(missing, Error::Protocol(m) if m.contains("is not set")));

        let empty = Bridge::from_env_value(Some(std::ffi::OsString::from(""))).unwrap_err();
        assert!(matches!(empty, Error::Protocol(m) if m.contains("empty")));

        let ok = Bridge::from_env_value(Some(std::ffi::OsString::from("/tmp/FMBridge")))
            .expect("parses a path");
        assert_eq!(ok.binary_path(), std::path::Path::new("/tmp/FMBridge"));
    }

    #[test]
    fn concurrency_limit_defaults_to_one_and_clamps_zero() {
        let bridge = Bridge::new("/tmp/FMBridge");
        assert_eq!(bridge.max_concurrency_limit(), DEFAULT_MAX_CONCURRENCY);
        assert_eq!(bridge.max_concurrency_limit(), 1);
        assert_eq!(bridge.available_slots(), 1);

        let clamped = bridge.clone().max_concurrency(0);
        assert_eq!(clamped.max_concurrency_limit(), 1);

        let raised = bridge.max_concurrency(8);
        assert_eq!(raised.max_concurrency_limit(), 8);
        assert_eq!(raised.available_slots(), 8);
    }
}

#[cfg(test)]
mod bundle_tests {
    use crate::Error;
    use crate::client::resolve_bundled;
    use std::fs;

    /// Builds `Root.app/Contents/{MacOS,Resources}` and returns the temp dir
    /// plus the `Contents/MacOS` path that stands in for `current_exe()`'s parent.
    fn fake_app() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let macos = dir.path().join("Root.app/Contents/MacOS");
        let resources = dir.path().join("Root.app/Contents/Resources");
        fs::create_dir_all(&macos).unwrap();
        fs::create_dir_all(&resources).unwrap();
        // The host app executable, which is what would be running.
        fs::write(macos.join("Root"), b"#!/bin/sh\n").unwrap();
        (dir, macos)
    }

    #[test]
    fn finds_helper_next_to_the_executable() {
        let (_dir, macos) = fake_app();
        let helper = macos.join("FMBridge");
        fs::write(&helper, b"#!/bin/sh\n").unwrap();

        let found = resolve_bundled(&macos).expect("helper in Contents/MacOS");
        assert_eq!(found, fs::canonicalize(&helper).unwrap());
    }

    #[test]
    fn falls_back_to_the_resources_directory() {
        let (dir, macos) = fake_app();
        let helper = dir.path().join("Root.app/Contents/Resources/FMBridge");
        fs::write(&helper, b"#!/bin/sh\n").unwrap();

        let found = resolve_bundled(&macos).expect("helper in Contents/Resources");
        assert_eq!(found, fs::canonicalize(&helper).unwrap());
        // The `..` segment must be resolved away, not left in the path.
        assert!(!found.to_string_lossy().contains(".."));
    }

    #[test]
    fn prefers_macos_over_resources_when_both_exist() {
        let (dir, macos) = fake_app();
        let preferred = macos.join("FMBridge");
        fs::write(&preferred, b"#!/bin/sh\n").unwrap();
        fs::write(
            dir.path().join("Root.app/Contents/Resources/FMBridge"),
            b"#!/bin/sh\n",
        )
        .unwrap();

        let found = resolve_bundled(&macos).unwrap();
        assert_eq!(found, fs::canonicalize(&preferred).unwrap());
    }

    #[test]
    fn reports_the_expected_location_when_absent() {
        let (_dir, macos) = fake_app();
        let error = resolve_bundled(&macos).unwrap_err();
        match error {
            Error::BinaryNotFound(path) => {
                assert_eq!(path, macos.join("FMBridge"));
            }
            other => panic!("expected BinaryNotFound, got {other:?}"),
        }
    }

    #[test]
    fn a_directory_named_like_the_helper_is_not_accepted() {
        let (_dir, macos) = fake_app();
        // A stray directory must not be mistaken for the executable.
        fs::create_dir(macos.join("FMBridge")).unwrap();
        assert!(matches!(
            resolve_bundled(&macos),
            Err(Error::BinaryNotFound(_))
        ));
    }
}
