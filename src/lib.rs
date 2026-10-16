#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]
#![warn(missing_docs, missing_debug_implementations, rust_2018_idioms)]

mod bridge;
mod error;
mod locate;
mod request;
mod response;
mod schema;
mod session;
mod wire;

pub use bridge::{Bridge, DEFAULT_MAX_CONCURRENCY, DEFAULT_TIMEOUT};
pub use error::{Error, Result, Unavailable};
pub use locate::BINARY_ENV_VAR;
pub use request::{Message, Request, Role, Sampling};
pub use response::{Availability, Completion, StreamEvent, Usage};
pub use schema::{MAX_DEPTH, Schema, SchemaProperty, SchemaType};
