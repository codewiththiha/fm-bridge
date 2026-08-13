//! Running several requests at once against one shared bridge.
//!
//! ```text
//! cargo run --example concurrent
//! cargo run --example concurrent -- 4        # allow 4 in flight
//! ```
//!
//! A bridge runs **one** request at a time unless you say otherwise, because
//! the on-device model is a single shared resource. `max_concurrency` raises
//! that ceiling; anything over the limit queues rather than failing, and every
//! caller gets the answer to its own prompt.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use fm_bridge::{Bridge, Request};

/// Prompts that will be answered in parallel.
const PROMPTS: &[&str] = &[
    "Name a seabird.",
    "Name a mountain range in Japan.",
    "Name a string instrument.",
    "Name a leafy green vegetable.",
    "Name a programming language from the 1970s.",
    "Name a constellation visible from Tokyo.",
    "Name a type of cloud.",
    "Name a board game older than 500 years.",
];

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // First CLI argument sets the limit; the default of 1 is fully serial.
    let limit: usize = std::env::args()
        .nth(1)
        .and_then(|arg| arg.parse().ok())
        .unwrap_or(4);

    let bridge = Bridge::discover()?
        .max_concurrency(limit)
        // The timeout covers queue time as well as generation, so a backlog
        // surfaces as `Error::Timeout` instead of an unbounded wait.
        .timeout(Duration::from_secs(120));

    bridge.check_availability().await?;

    println!(
        "Sending {} prompts through a bridge limited to {} at a time.\n",
        PROMPTS.len(),
        bridge.max_concurrency_limit()
    );

    // Samples how many slots are actually in use, to show the limit holding.
    // Counting at the call site would just count *queued* tasks — the useful
    // number is how many helper processes the bridge is really running.
    let peak = Arc::new(AtomicUsize::new(0));
    let sampler = {
        let bridge = bridge.clone();
        let peak = Arc::clone(&peak);
        tokio::spawn(async move {
            let limit = bridge.max_concurrency_limit();
            loop {
                let busy = limit - bridge.available_slots();
                peak.fetch_max(busy, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
    };

    let started = Instant::now();

    let tasks: Vec<_> = PROMPTS
        .iter()
        .enumerate()
        .map(|(index, prompt)| {
            // Clones share the parent's budget, so spawning freely is safe.
            let bridge = bridge.clone();

            tokio::spawn(async move {
                let request = Request::new()
                    .system("Answer with just the name. No punctuation, no explanation.")
                    .user(*prompt)
                    .max_tokens(16);

                let result = bridge.complete(request).await;
                (index, *prompt, result)
            })
        })
        .collect();

    // Collect into a fixed-size slot per task so output order matches the
    // prompt order even though completion order will not.
    let mut answers: Vec<Option<String>> = vec![None; PROMPTS.len()];
    let mut failures = 0usize;

    for task in tasks {
        let (index, prompt, result) = task.await?;
        match result {
            Ok(completion) => {
                answers[index] = Some(completion.text.trim().to_string());
            }
            Err(error) => {
                failures += 1;
                // A saturated bridge reports a retryable timeout; other errors
                // (guardrails, context limits) are per-request and final.
                let hint = if error.is_retryable() {
                    " (retryable)"
                } else {
                    ""
                };
                eprintln!("[{index}] {prompt} -> failed{hint}: {error}");
            }
        }
    }

    sampler.abort();

    for (index, prompt) in PROMPTS.iter().enumerate() {
        match &answers[index] {
            Some(answer) => println!("{prompt}\n  -> {answer}"),
            None => println!("{prompt}\n  -> (no answer)"),
        }
    }

    println!(
        "\nFinished {} of {} prompts in {:.1}s; peak concurrent requests: {} (limit {}).",
        PROMPTS.len() - failures,
        PROMPTS.len(),
        started.elapsed().as_secs_f64(),
        peak.load(Ordering::SeqCst),
        bridge.max_concurrency_limit()
    );
    println!("All slots returned: {} free.", bridge.available_slots());

    Ok(())
}
