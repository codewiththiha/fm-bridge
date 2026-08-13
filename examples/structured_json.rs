//! Generates a JSON object that is guaranteed to match a runtime-defined schema.
//!
//! ```text
//! FM_BRIDGE_BIN=swift/.build/release/FMBridge cargo run --example structured_json
//! ```

use fm_bridge::{Bridge, Request, Schema, SchemaProperty};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct Review {
    title: String,
    sentiment: String,
    rating: i64,
    pros: Vec<String>,
    cons: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let bridge = Bridge::discover()?;

    let schema = Schema::new(
        "Review",
        vec![
            SchemaProperty::string("title").description("A short headline for the review"),
            SchemaProperty::string("sentiment")
                .description("Overall tone")
                .any_of(["positive", "mixed", "negative"]),
            SchemaProperty::integer("rating")
                .description("Score out of five")
                .range(1.0, 5.0),
            SchemaProperty::array("pros", SchemaProperty::string("point")).count(1, 3),
            SchemaProperty::array("cons", SchemaProperty::string("point")).count(1, 3),
        ],
    )
    .description("A structured product review");

    let response = bridge
        .complete(
            Request::new()
                .user(
                    "Write a review of a mechanical keyboard that is loud but very pleasant \
                     to type on.",
                )
                .schema(schema),
        )
        .await?;

    let review: Review = response.parse()?;

    println!(
        "{} — {} ({}/5)",
        review.title, review.sentiment, review.rating
    );
    for pro in &review.pros {
        println!("  + {pro}");
    }
    for con in &review.cons {
        println!("  - {con}");
    }

    Ok(())
}
