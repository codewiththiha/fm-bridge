//! Streams a response token-by-token as the model produces it.
//!
//! ```text
//! swift build -c release --package-path swift
//! FM_BRIDGE_BIN=swift/.build/release/FMBridge cargo run --example stream_text
//! ```

use std::io::Write;

use fm_bridge::{Bridge, Request, StreamEvent};
use futures::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bridge = Bridge::discover()?;
    bridge.check_availability().await?;

    let request = Request::new()
        .system("You are a helpful assistant who answers in vivid but compact prose.")
        .user("Describe a thunderstorm rolling over Seoul at night.")
        .temperature(0.8);

    let mut stream = Box::pin(bridge.stream(request));

    while let Some(event) = stream.next().await {
        match event? {
            StreamEvent::Delta(text) => {
                print!("{text}");
                std::io::stdout().flush()?;
            }
            StreamEvent::Done(usage) => {
                println!(
                    "\n\n[~{} prompt tokens, ~{} completion tokens (estimated)]",
                    usage.prompt_tokens, usage.completion_tokens
                );
            }
            other => eprintln!("\n[unexpected event: {other:?}]"),
        }
    }

    Ok(())
}
