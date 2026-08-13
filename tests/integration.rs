//! End-to-end tests against mock helper binaries.
//!
//! Apple Intelligence requires real Apple Silicon hardware with the model
//! downloaded, which no CI runner has. These tests therefore drive the real
//! IPC code path — spawn, stdin write, NDJSON parse, exit-status handling —
//! against bash scripts in `scripts/` that reproduce the helper's stdout
//! byte-for-byte.

use std::path::PathBuf;
use std::time::Duration;

use fm_bridge::{
    Bridge, Completion, Error, Request, Sampling, Schema, SchemaProperty, StreamEvent,
};
use futures::StreamExt;

fn mock(name: &str) -> Bridge {
    let path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "scripts", name]
        .iter()
        .collect();
    assert!(path.is_file(), "missing mock script: {}", path.display());
    Bridge::new(path)
}

/// Writes a wrapper script that runs `script` with extra environment variables,
/// so tests stay parallel-safe instead of mutating the process environment.
fn mock_with_env(script: &str, vars: &[(&str, &str)]) -> (tempfile::TempDir, Bridge) {
    let dir = tempfile::tempdir().expect("create tempdir");
    let wrapper = dir.path().join("wrapper.sh");
    let target: PathBuf = [env!("CARGO_MANIFEST_DIR"), "scripts", script]
        .iter()
        .collect();

    let exports: String = vars
        .iter()
        .map(|(key, value)| format!("export {key}='{value}'\n"))
        .collect();
    let body = format!(
        "#!/usr/bin/env bash\n{exports}exec {} \"$@\"\n",
        target.display()
    );

    std::fs::write(&wrapper, body).expect("write wrapper");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755))
            .expect("chmod wrapper");
    }

    let bridge = Bridge::new(&wrapper);
    (dir, bridge)
}

async fn collect(bridge: &Bridge, request: Request) -> Vec<StreamEvent> {
    let mut stream = Box::pin(bridge.stream(request));
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event.expect("stream event"));
    }
    events
}

// ── Text ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn complete_concatenates_streamed_deltas() {
    let bridge = mock("mock_text_stream.sh");
    let response = bridge
        .complete(Request::new().user("hi"))
        .await
        .expect("completion should succeed");

    assert_eq!(response.text, "Hello, world!");
    assert_eq!(response.usage.prompt_tokens, 7);
    assert_eq!(response.usage.completion_tokens, 4);
    assert_eq!(response.usage.total_tokens(), 11);
    assert!(response.structured.is_none());
}

#[tokio::test]
async fn stream_yields_deltas_in_order_then_done() {
    let bridge = mock("mock_text_stream.sh");
    let events = collect(&bridge, Request::new().user("hi")).await;

    let deltas: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            StreamEvent::Delta(text) => Some(text.as_str()),
            _ => None,
        })
        .collect();

    assert_eq!(deltas, ["Hello", ", ", "world", "!"]);
    assert!(matches!(events.last(), Some(StreamEvent::Done(_))));
}

#[tokio::test]
async fn ignores_unknown_events_and_stray_stdout() {
    let bridge = mock("mock_noise.sh");
    let response = bridge
        .complete(Request::new().user("hi"))
        .await
        .expect("completion");

    // The `objc[...]` warning, the blank line, the `ready` event, and the
    // unknown `someFutureEvent` must all be skipped.
    assert_eq!(response.text, "real text");
}

// ── Structured ──────────────────────────────────────────────────────────────

fn recipe_schema() -> Schema {
    Schema::new(
        "Recipe",
        vec![
            SchemaProperty::string("title"),
            SchemaProperty::integer("minutes").range(1.0, 240.0),
            SchemaProperty::array("ingredients", SchemaProperty::string("item")).count(1, 10),
        ],
    )
}

#[tokio::test]
async fn complete_returns_structured_payload() {
    let bridge = mock("mock_structured.sh");
    let response = bridge
        .complete(
            Request::new()
                .user("a pasta recipe")
                .schema(recipe_schema()),
        )
        .await
        .expect("completion");

    let structured = response.structured.as_ref().expect("structured payload");
    assert_eq!(structured["title"], "Cacio e Pepe");
    assert_eq!(structured["minutes"], 15);
    assert_eq!(structured["ingredients"].as_array().unwrap().len(), 3);
    assert!(response.text.is_empty());
}

#[tokio::test]
async fn structured_payload_deserializes_into_a_struct() {
    #[derive(serde::Deserialize)]
    struct Recipe {
        title: String,
        minutes: u32,
        ingredients: Vec<String>,
    }

    let bridge = mock("mock_structured.sh");
    let response = bridge
        .complete(
            Request::new()
                .user("a pasta recipe")
                .schema(recipe_schema()),
        )
        .await
        .expect("completion");

    let recipe: Recipe = response.parse().expect("deserialize");
    assert_eq!(recipe.title, "Cacio e Pepe");
    assert_eq!(recipe.minutes, 15);
    assert_eq!(recipe.ingredients[0], "pasta");
}

#[tokio::test]
async fn structured_streaming_emits_snapshots_then_final_object() {
    let bridge = mock("mock_structured_stream.sh");
    let events = collect(
        &bridge,
        Request::new()
            .user("a pasta recipe")
            .schema(recipe_schema())
            .stream_structured(true),
    )
    .await;

    let snapshots: Vec<_> = events
        .iter()
        .filter(|event| matches!(event, StreamEvent::Snapshot(_)))
        .collect();
    assert_eq!(snapshots.len(), 2, "expected two partial snapshots");

    let Some(StreamEvent::Structured(final_object)) = events
        .iter()
        .find(|event| matches!(event, StreamEvent::Structured(_)))
    else {
        panic!("no final structured event in {events:?}");
    };
    assert_eq!(final_object["title"], "Cacio e Pepe");
    assert!(matches!(events.last(), Some(StreamEvent::Done(_))));
}

// ── Wire format ─────────────────────────────────────────────────────────────

/// Round-trips a request through the echo mock to assert on the exact bytes
/// the crate writes to the helper's stdin.
async fn echoed_request(request: Request) -> serde_json::Value {
    let bridge = mock("mock_echo_request.sh");
    let response: Completion = bridge.complete(request).await.expect("completion");
    serde_json::from_str(&response.text).expect("echoed request should be valid JSON")
}

#[tokio::test]
async fn writes_expected_wire_format_for_text_requests() {
    let sent = echoed_request(
        Request::new()
            .system("be brief")
            .user("hello")
            .temperature(0.3)
            .max_tokens(256)
            .sampling(Sampling::TopK {
                top: 20,
                seed: Some(99),
            }),
    )
    .await;

    assert_eq!(sent["messages"][0]["role"], "system");
    assert_eq!(sent["messages"][0]["content"], "be brief");
    assert_eq!(sent["messages"][1]["role"], "user");
    assert_eq!(sent["stream"], true);
    assert_eq!(sent["temperature"], 0.3);
    assert_eq!(sent["maxTokens"], 256);
    assert_eq!(sent["topK"], 20);
    assert_eq!(sent["seed"], 99);
    assert!(sent.get("schema").is_none());
}

#[tokio::test]
async fn writes_expected_wire_format_for_structured_requests() {
    let sent = echoed_request(Request::new().user("hi").schema(recipe_schema())).await;

    // Non-streaming structured requests must not ask the helper to stream.
    assert_eq!(sent["stream"], false);
    assert_eq!(sent["schema"]["name"], "Recipe");
    assert_eq!(sent["schema"]["properties"][1]["type"], "integer");
    assert_eq!(sent["schema"]["properties"][1]["range"][1], 240.0);
    assert_eq!(sent["schema"]["properties"][2]["items"]["type"], "string");
    assert!(sent.get("streamStructured").is_none());
}

#[tokio::test]
async fn greedy_sampling_is_sent_as_a_flag() {
    let sent = echoed_request(Request::new().user("hi").sampling(Sampling::Greedy)).await;

    assert_eq!(sent["greedy"], true);
    assert!(sent.get("topK").is_none());
    assert!(sent.get("seed").is_none());
}

// ── Errors ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn typed_error_codes_map_to_error_variants() {
    let cases = [
        ("model_unavailable", "Apple Intelligence is turned off."),
        ("bad_request", "empty prompt"),
        ("schema_invalid", "duplicate property name"),
        ("guardrail_violation", "blocked"),
        ("context_exceeded", "too long"),
    ];

    for (code, message) in cases {
        let (_dir, bridge) = mock_with_env(
            "mock_error.sh",
            &[("MOCK_ERROR_CODE", code), ("MOCK_ERROR_MESSAGE", message)],
        );
        let error = bridge
            .complete(Request::new().user("hi"))
            .await
            .unwrap_err();

        let matched = match (code, &error) {
            ("model_unavailable", Error::ModelUnavailable(m)) => m == message,
            ("bad_request", Error::BadRequest(m)) => m == message,
            ("schema_invalid", Error::InvalidSchema(m)) => m == message,
            ("guardrail_violation", Error::GuardrailViolation(m)) => m == message,
            ("context_exceeded", Error::ContextExceeded(m)) => m == message,
            _ => false,
        };
        assert!(matched, "code {code} produced {error:?}");
    }
}

#[tokio::test]
async fn unknown_error_codes_degrade_to_generation_errors() {
    let (_dir, bridge) = mock_with_env(
        "mock_error.sh",
        &[
            ("MOCK_ERROR_CODE", "some_future_code"),
            ("MOCK_ERROR_MESSAGE", "who knows"),
        ],
    );
    let error = bridge
        .complete(Request::new().user("hi"))
        .await
        .unwrap_err();

    assert!(matches!(error, Error::Generation(m) if m == "who knows"));
}

#[tokio::test]
async fn errors_surface_through_the_stream_api_too() {
    let (_dir, bridge) = mock_with_env(
        "mock_error.sh",
        &[
            ("MOCK_ERROR_CODE", "guardrail_violation"),
            ("MOCK_ERROR_MESSAGE", "nope"),
        ],
    );

    let mut stream = Box::pin(bridge.stream(Request::new().user("hi")));
    let first = stream.next().await.expect("one item").unwrap_err();
    assert!(matches!(first, Error::GuardrailViolation(_)));
}

#[tokio::test]
async fn crash_before_completion_reports_exit_status_and_stderr() {
    let bridge = mock("mock_crash.sh");
    let error = bridge
        .complete(Request::new().user("hi"))
        .await
        .unwrap_err();

    match error {
        Error::ProcessFailed { status, stderr } => {
            assert!(status.contains('9'), "unexpected status: {status}");
            assert!(
                stderr.contains("FoundationModels"),
                "unexpected stderr: {stderr}"
            );
        }
        other => panic!("expected ProcessFailed, got {other:?}"),
    }
}

#[tokio::test]
async fn missing_binary_is_reported_clearly() {
    let bridge = Bridge::new("/nonexistent/path/to/FMBridge");

    let error = bridge
        .complete(Request::new().user("hi"))
        .await
        .unwrap_err();
    assert!(matches!(error, Error::BinaryNotFound(_)));

    let mut stream = Box::pin(bridge.stream(Request::new().user("hi")));
    let streamed = stream.next().await.expect("one item").unwrap_err();
    assert!(matches!(streamed, Error::BinaryNotFound(_)));
}

#[tokio::test]
async fn requests_are_validated_before_spawning() {
    // Validation must fail even though the binary path is bogus, proving no
    // process is spawned for a malformed request.
    let bridge = Bridge::new("/nonexistent/path/to/FMBridge");

    let no_user_turn = bridge
        .complete(Request::new().system("hi"))
        .await
        .unwrap_err();
    assert!(matches!(no_user_turn, Error::BadRequest(_)));

    let bad_schema = bridge
        .complete(
            Request::new()
                .user("hi")
                .schema(Schema::new("Empty", vec![])),
        )
        .await
        .unwrap_err();
    assert!(matches!(bad_schema, Error::InvalidSchema(_)));
}

#[tokio::test]
async fn slow_helpers_hit_the_timeout() {
    let (_dir, bridge) = mock_with_env("mock_slow.sh", &[("MOCK_SLEEP_SECONDS", "30")]);
    let bridge = bridge.timeout(Duration::from_millis(300));

    let error = bridge
        .complete(Request::new().user("hi"))
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Timeout(_)));
    assert!(error.is_retryable());

    let mut stream = Box::pin(bridge.stream(Request::new().user("hi")));
    let mut last = None;
    while let Some(event) = stream.next().await {
        match event {
            Ok(event) => last = Some(Ok(event)),
            Err(error) => {
                last = Some(Err(error));
                break;
            }
        }
    }
    assert!(matches!(last, Some(Err(Error::Timeout(_)))), "got {last:?}");
}

// ── Availability probe ──────────────────────────────────────────────────────

#[tokio::test]
async fn probe_succeeds_when_the_model_is_ready() {
    let bridge = mock("mock_probe.sh");
    bridge
        .check_availability()
        .await
        .expect("probe should succeed");
}

#[tokio::test]
async fn probe_reports_why_the_model_is_unavailable() {
    let (_dir, bridge) = mock_with_env("mock_probe.sh", &[("MOCK_UNAVAILABLE", "1")]);
    let error = bridge.check_availability().await.unwrap_err();

    match error {
        Error::ModelUnavailable(message) => assert!(message.contains("System Settings")),
        other => panic!("expected ModelUnavailable, got {other:?}"),
    }
}

// ── Process hygiene ─────────────────────────────────────────────────────────

#[tokio::test]
async fn dropping_a_stream_early_does_not_leak_the_child() {
    let (_dir, bridge) = mock_with_env("mock_slow.sh", &[("MOCK_SLEEP_SECONDS", "120")]);

    let mut stream = Box::pin(bridge.stream(Request::new().user("hi")));
    let first = stream.next().await.expect("first event").expect("ok");
    assert_eq!(first, StreamEvent::Delta("tick".into()));

    // `kill_on_drop(true)` must reap the helper as soon as the stream is gone.
    drop(stream);
    tokio::time::sleep(Duration::from_millis(200)).await;

    let survivors = std::process::Command::new("pgrep")
        .args(["-f", "MOCK_SLEEP_SECONDS"])
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_default();
    assert!(
        survivors.is_empty(),
        "helper survived stream drop: {survivors}"
    );
}

#[tokio::test]
async fn concurrent_requests_are_independent() {
    let bridge = mock("mock_text_stream.sh");

    let futures = (0..8).map(|_| {
        let bridge = bridge.clone();
        tokio::spawn(async move { bridge.complete(Request::new().user("hi")).await })
    });

    for handle in futures {
        let response = handle.await.expect("task").expect("completion");
        assert_eq!(response.text, "Hello, world!");
    }
}

/// `from_env` is exercised in a child process rather than by mutating this
/// one's environment: `set_var` is `unsafe` in edition 2024 (it can race with a
/// concurrent `getenv` in another thread), and this crate forbids `unsafe`.
/// Spawning gives the same coverage with none of the hazard, and additionally
/// proves the variable is read at the process boundary the way callers use it.
#[tokio::test]
async fn from_env_reads_the_binary_path() {
    let mock: PathBuf = [env!("CARGO_MANIFEST_DIR"), "scripts", "mock_text_stream.sh"]
        .iter()
        .collect();

    // Re-runs this same test binary with the variable set, filtered down to the
    // helper test below, which is `#[ignore]`d during a normal run.
    let output = std::process::Command::new(std::env::current_exe().expect("test binary"))
        .args(["--exact", "env_var_helper", "--ignored", "--nocapture"])
        .env(fm_bridge::BINARY_ENV_VAR, &mock)
        .output()
        .expect("re-run test binary");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "child failed: {stdout}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains(&format!("resolved={}", mock.display())),
        "child did not resolve the env var: {stdout}"
    );
}

/// Helper for [`from_env_reads_the_binary_path`]; only meaningful with
/// `FM_BRIDGE_BIN` set, so it is skipped in ordinary runs.
#[tokio::test]
#[ignore = "spawned by from_env_reads_the_binary_path with the env var set"]
async fn env_var_helper() {
    let bridge = Bridge::from_env().expect("from_env");
    println!("resolved={}", bridge.binary_path().display());

    // The path really is usable, not just parsed.
    let response = bridge
        .complete(Request::new().user("hi"))
        .await
        .expect("completion");
    assert_eq!(response.text, "Hello, world!");
}

// ── Concurrency ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn concurrency_defaults_to_one() {
    let bridge = mock("mock_text_stream.sh");
    assert_eq!(bridge.max_concurrency_limit(), 1);
    assert_eq!(
        bridge.max_concurrency_limit(),
        fm_bridge::DEFAULT_MAX_CONCURRENCY
    );
    assert_eq!(bridge.available_slots(), 1);
}

#[tokio::test]
async fn zero_concurrency_is_clamped_to_one() {
    let bridge = mock("mock_text_stream.sh").max_concurrency(0);
    assert_eq!(bridge.max_concurrency_limit(), 1);

    // Still functional rather than deadlocked.
    let response = bridge
        .complete(Request::new().user("hi"))
        .await
        .expect("completion");
    assert_eq!(response.text, "Hello, world!");
}

#[tokio::test]
async fn the_default_limit_serializes_requests() {
    // Each helper reports its own start/end, so overlap is observable.
    let (_dir, bridge) = mock_with_env("mock_concurrency_probe.sh", &[("MOCK_HOLD_MS", "150")]);

    let started = std::time::Instant::now();
    let handles: Vec<_> = (0..3)
        .map(|_| {
            let bridge = bridge.clone();
            tokio::spawn(async move { bridge.complete(Request::new().user("hi")).await })
        })
        .collect();

    for handle in handles {
        handle.await.expect("task").expect("completion");
    }

    // Three 150 ms requests run one at a time: comfortably over 300 ms.
    assert!(
        started.elapsed() >= Duration::from_millis(300),
        "requests appear to have overlapped under the default limit of 1 ({:?})",
        started.elapsed()
    );
}

#[tokio::test]
async fn raising_the_limit_allows_overlap() {
    let (_dir, bridge) = mock_with_env("mock_concurrency_probe.sh", &[("MOCK_HOLD_MS", "300")]);
    let bridge = bridge.max_concurrency(4);
    assert_eq!(bridge.available_slots(), 4);

    let started = std::time::Instant::now();
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let bridge = bridge.clone();
            tokio::spawn(async move { bridge.complete(Request::new().user("hi")).await })
        })
        .collect();

    for handle in handles {
        handle.await.expect("task").expect("completion");
    }

    // Four 300 ms requests in parallel finish in far less than 1.2 s.
    assert!(
        started.elapsed() < Duration::from_millis(900),
        "requests did not overlap despite a limit of 4 ({:?})",
        started.elapsed()
    );
}

#[tokio::test]
async fn clones_share_one_concurrency_budget() {
    let (_dir, bridge) = mock_with_env("mock_concurrency_probe.sh", &[("MOCK_HOLD_MS", "200")]);
    let bridge = bridge.max_concurrency(2);

    let a = bridge.clone();
    let b = bridge.clone();
    let c = bridge.clone();

    let started = std::time::Instant::now();
    let handles = vec![
        tokio::spawn(async move { a.complete(Request::new().user("1")).await }),
        tokio::spawn(async move { b.complete(Request::new().user("2")).await }),
        tokio::spawn(async move { c.complete(Request::new().user("3")).await }),
    ];
    for handle in handles {
        handle.await.expect("task").expect("completion");
    }

    // Three requests through a shared 2-slot budget need two waves, so the
    // clones cannot each have got their own budget.
    assert!(
        started.elapsed() >= Duration::from_millis(400),
        "clones did not share the limit ({:?})",
        started.elapsed()
    );
}

#[tokio::test]
async fn every_response_reaches_its_own_caller() {
    // The strongest guarantee to hold under concurrency: with many requests in
    // flight through one bridge, each caller must receive the reply to *its*
    // prompt. The mock echoes back the request it was handed.
    let bridge = mock("mock_echo_request.sh").max_concurrency(4);

    let handles: Vec<_> = (0..24)
        .map(|index| {
            let bridge = bridge.clone();
            tokio::spawn(async move {
                let marker = format!("prompt-{index}");
                let response = bridge
                    .complete(Request::new().user(&marker))
                    .await
                    .expect("completion");
                (marker, response.text)
            })
        })
        .collect();

    for handle in handles {
        let (marker, text) = handle.await.expect("task");
        let echoed: serde_json::Value = serde_json::from_str(&text).expect("echoed request");
        let content = echoed["messages"][0]["content"]
            .as_str()
            .expect("content")
            .to_string();
        assert_eq!(
            content, marker,
            "a caller received another caller's response"
        );
    }
}

#[tokio::test]
async fn streams_release_their_slot_when_dropped() {
    let (_dir, bridge) = mock_with_env("mock_slow.sh", &[("MOCK_SLEEP_SECONDS", "120")]);
    let bridge = bridge.max_concurrency(1);

    let mut stream = Box::pin(bridge.stream(Request::new().user("hi")));
    stream.next().await.expect("first event").expect("ok");
    assert_eq!(bridge.available_slots(), 0, "stream should hold the slot");

    // Abandoning the stream must hand the slot back, or the bridge would wedge
    // permanently after any cancelled request.
    drop(stream);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        bridge.available_slots(),
        1,
        "dropping a stream must release its slot"
    );
}

#[tokio::test]
async fn a_saturated_bridge_times_out_instead_of_hanging() {
    let (_dir, bridge) = mock_with_env("mock_slow.sh", &[("MOCK_SLEEP_SECONDS", "30")]);
    let bridge = bridge
        .max_concurrency(1)
        .timeout(Duration::from_millis(400));

    // Occupy the single slot with a request that will not finish.
    let mut held = Box::pin(bridge.stream(Request::new().user("first")));
    held.next().await.expect("first event").expect("ok");

    // A second caller waits for the slot and gives up at the timeout rather
    // than queueing indefinitely.
    let started = std::time::Instant::now();
    let error = bridge
        .complete(Request::new().user("second"))
        .await
        .unwrap_err();

    assert!(
        matches!(error, Error::Timeout(_)),
        "expected a timeout while queued, got {error:?}"
    );
    assert!(error.is_retryable());
    assert!(started.elapsed() >= Duration::from_millis(300));
}

#[tokio::test]
async fn a_freed_slot_is_handed_to_the_next_waiter() {
    let (_dir, bridge) = mock_with_env("mock_concurrency_probe.sh", &[("MOCK_HOLD_MS", "200")]);
    let bridge = bridge.max_concurrency(1).timeout(Duration::from_secs(10));

    let first = {
        let bridge = bridge.clone();
        tokio::spawn(async move { bridge.complete(Request::new().user("first")).await })
    };

    // Give the first request time to take the only slot.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let second = {
        let bridge = bridge.clone();
        tokio::spawn(async move { bridge.complete(Request::new().user("second")).await })
    };

    first.await.expect("task").expect("first completion");
    second.await.expect("task").expect("second completion");

    // Both finished, and the budget is fully restored.
    assert_eq!(bridge.available_slots(), 1);
}

#[tokio::test]
async fn streaming_a_structured_request_requires_opting_in() {
    let bridge = mock("mock_structured.sh");

    let mut stream = Box::pin(bridge.stream(Request::new().user("hi").schema(recipe_schema())));
    let error = stream.next().await.expect("one item").unwrap_err();
    assert!(matches!(error, Error::BadRequest(m) if m.contains("stream_structured")));

    // With the opt-in it streams normally.
    let bridge = mock("mock_structured_stream.sh");
    let events = collect(
        &bridge,
        Request::new()
            .user("hi")
            .schema(recipe_schema())
            .stream_structured(true),
    )
    .await;
    assert!(events.iter().any(|e| matches!(e, StreamEvent::Snapshot(_))));
}
