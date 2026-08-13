//! A multi-turn REPL that keeps conversation history in memory.
//!
//! ```text
//! FM_BRIDGE_BIN=swift/.build/release/FMBridge cargo run --example chat
//! ```
//!
//! Each request spawns a fresh helper process, so history must be replayed on
//! every turn — which is exactly what [`Request::messages`] is for.

use std::io::{BufRead, Write};

use fm_bridge::{Bridge, Message, Request, StreamEvent};
use futures::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bridge = Bridge::discover()?;
    bridge.check_availability().await?;

    let mut history = vec![Message::system(
        "You are a friendly assistant. Keep answers under four sentences.",
    )];

    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();

    println!("Chatting with the on-device model. Press Ctrl-D to quit.\n");

    loop {
        print!("you> ");
        std::io::stdout().flush()?;

        let Some(line) = lines.next().transpose()? else {
            break;
        };
        let prompt = line.trim();
        if prompt.is_empty() {
            continue;
        }

        history.push(Message::user(prompt));

        print!("model> ");
        std::io::stdout().flush()?;

        let mut reply = String::new();
        let mut stream = Box::pin(bridge.stream(Request::new().messages(history.clone())));

        while let Some(event) = stream.next().await {
            match event? {
                StreamEvent::Delta(delta) => {
                    print!("{delta}");
                    std::io::stdout().flush()?;
                    reply.push_str(&delta);
                }
                StreamEvent::Done(_) => println!("\n"),
                _ => {}
            }
        }

        history.push(Message::assistant(reply));
    }

    println!("\nBye.");
    Ok(())
}
