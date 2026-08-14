# fm-bridge

Async Rust bindings for Apple's on-device **Foundation Models** framework — the same ~3B parameter model that powers Apple Intelligence.

Apple ships `FoundationModels` as a Swift-only framework with no C ABI, so this crate pairs a small Swift helper binary with a pure Rust library. They speak newline-delimited JSON over stdin/stdout.

```rust
use fm_bridge::{Bridge, Request};

let bridge = Bridge::from_env()?;
let response = bridge
    .complete(Request::new().user("Name three seabirds."))
    .await?;

println!("{}", response.text);
```

## Features

- **Text generation** — buffered via `complete()` or incremental via `stream()`
- **Structured output** — runtime-defined JSON schemas with constrained decoding, so responses are *guaranteed* well-formed
- **Streaming structured output** — partial snapshots as the object fills in
- **Typed errors** — guardrail violations, context overflow, model unavailability, and schema problems are distinct variants
- **Bounded concurrency** — serial by default, `max_concurrency(n)` to fan out; extra requests queue instead of failing
- **No leaked processes** — `kill_on_drop(true)` plus per-request timeouts
- **Runs offline, free, and private** — inference never leaves the device

## Requirements

| | |
|---|---|
| Hardware | Apple Silicon Mac (M1 or later) |
| OS | macOS 26 (Tahoe) or later |
| Setting | Apple Intelligence enabled in System Settings |
| Toolchain | Swift 6.2+ (Xcode 26) to build the helper |
| Rust | 1.85 or later (the crate uses edition 2024) |

The model is not available on Intel Macs, and it will report itself unavailable while macOS is still downloading it.

## Installation

```toml
[dependencies]
fm-bridge = "0.1"
tokio = { version = "1", features = ["full"] }
```

### Build the Swift helper

```bash
swift build -c release --package-path swift
export FM_BRIDGE_BIN="$PWD/swift/.build/release/FMBridge"
```

Then locate it from Rust:

```rust
let bridge = Bridge::from_env()?;   // reads FM_BRIDGE_BIN
let bridge = Bridge::discover()?;   // env var, then swift/.build, then PATH
let bridge = Bridge::new("/path/to/FMBridge");
```

Check the model is actually usable before you rely on it:

```rust
match bridge.check_availability().await {
    Ok(()) => println!("ready"),
    Err(error) => eprintln!("unavailable: {error}"),
}
```

When it fails, branch on the *reason* rather than the message — the wording is
for humans and may change, the token will not:

```rust
use fm_bridge::{Error, Unavailable};

match bridge.check_availability().await {
    Ok(()) => { /* go ahead */ }
    Err(Error::ModelUnavailable { reason, message }) => match reason {
        // Transient: assets are still coming down. Back off and retry.
        Unavailable::ModelNotReady => schedule_retry(),
        // Actionable by the user.
        Unavailable::NotEnabled => prompt_user_to_enable_apple_intelligence(),
        // Permanent on this machine — hide the feature entirely.
        Unavailable::DeviceNotEligible | Unavailable::OsTooOld => hide_ai_features(),
        _ => eprintln!("unavailable: {message}"),
    },
    Err(error) => eprintln!("{error}"),
}
```

`Error::is_retryable()` already encodes this distinction, so
`Unavailable::ModelNotReady` reports `true` while an ineligible device reports
`false`.

## Usage

### Streaming text

```rust
use fm_bridge::{Bridge, Request, StreamEvent};
use futures::StreamExt;

let bridge = Bridge::discover()?;
let mut stream = Box::pin(bridge.stream(Request::new().user("Write a haiku about rain.")));

while let Some(event) = stream.next().await {
    match event? {
        StreamEvent::Delta(text) => print!("{text}"),
        StreamEvent::Done(usage) => println!("\n~{} tokens", usage.total_tokens()),
        _ => {}
    }
}
```

`StreamEvent::Delta` carries only the *new* characters. Apple's framework emits cumulative snapshots; the Swift helper diffs them so Rust receives true deltas.

### Structured output

```rust
use fm_bridge::{Request, Schema, SchemaProperty};
use serde::Deserialize;

#[derive(Deserialize)]
struct Recipe {
    title: String,
    minutes: i64,
    ingredients: Vec<String>,
}

let schema = Schema::new(
    "Recipe",
    vec![
        SchemaProperty::string("title").description("Name of the dish"),
        SchemaProperty::integer("minutes").description("Total cook time").range(1.0, 240.0),
        SchemaProperty::array("ingredients", SchemaProperty::string("item")).count(2, 10),
    ],
);

let response = bridge
    .complete(Request::new().user("Invent a quick pasta dish.").schema(schema))
    .await?;

let recipe: Recipe = response.parse()?;
```

Descriptions are not decoration — they are the main lever you have for steering field content.

#### Supported constraints

| Builder | Applies to | Effect |
|---|---|---|
| `.description(..)` | all | Natural-language guidance |
| `.optional()` | all | Model may omit the field |
| `.range(min, max)` | integer, number | Inclusive numeric bounds |
| `.any_of([..])` | string | Restricts to a fixed set |
| `.pattern("..")` | string | Must match a regex |
| `.count(min, max)` | array | Bounds element count |

Nest with `SchemaProperty::object(name, props)` and `SchemaProperty::array(name, items)`.

### Streaming structured output

Structured requests do **not** stream by default — `stream()` returns `Error::BadRequest` unless you opt in:

```rust
let request = Request::new().user("Invent a dish.").schema(schema).stream_structured(true);

while let Some(event) = stream.next().await {
    match event? {
        StreamEvent::Snapshot(partial) => println!("partial: {partial}"),
        StreamEvent::Structured(done) => println!("final: {done}"),
        _ => {}
    }
}
```

Snapshots are partially-filled objects: fields not yet generated are absent or null.

### Multi-turn conversations

Each request spawns a fresh helper process, so there is no server-side session. Replay the history yourself:

```rust
let mut history = vec![Message::system("You are terse.")];
history.push(Message::user("What's the capital of France?"));

let response = bridge.complete(Request::new().messages(history.clone())).await?;
history.push(Message::assistant(response.text));
```

`Role::System` messages map onto Apple's `Instructions`; everything else becomes prompt turns.

### Generation options

```rust
use fm_bridge::{Request, Sampling};

Request::new()
    .user("Pick a number.")
    .temperature(0.2)
    .max_tokens(200)
    .sampling(Sampling::Greedy);                              // deterministic
    // .sampling(Sampling::TopK { top: 40, seed: Some(42) }); // reproducible
```

Set a per-request wall-clock limit on the bridge itself (default five minutes):

```rust
let bridge = Bridge::discover()?.timeout(std::time::Duration::from_secs(30));
```

### Concurrency

A bridge runs **one request at a time** by default. The on-device model is a single shared resource, so fanning out is something you ask for rather than something you get by accident:

```rust
let bridge = Bridge::discover()?.max_concurrency(4);

let tasks: Vec<_> = prompts
    .into_iter()
    .map(|prompt| {
        let bridge = bridge.clone();          // clones share the same 4 slots
        tokio::spawn(async move { bridge.complete(Request::new().user(prompt)).await })
    })
    .collect();

for task in tasks {
    println!("{}", task.await??.text);
}
```

The rules:

| Behaviour | What happens |
| --- | --- |
| Default limit | `1` — strictly serial (`DEFAULT_MAX_CONCURRENCY`) |
| Over the limit | Requests **queue** in roughly arrival order; nothing is rejected |
| Response routing | Each request gets its own helper process, so a reply can never reach the wrong caller |
| Queue time | Counts toward `timeout()`, so a saturated bridge returns a retryable `Error::Timeout` instead of hanging |
| Clones | Share one budget — `bridge.clone()` does **not** multiply your slots |
| Cancellation | Dropping a future or stream frees its slot immediately |
| `max_concurrency(0)` | Clamped to `1` rather than deadlocking |
| Scope | `complete()`, `stream()`, and `check_availability()` all take a slot |

Inspect the budget at runtime with `bridge.max_concurrency_limit()` and `bridge.available_slots()`.

Keep the number modest. Each slot is a separate helper process contending for one on-device model, so throughput does not scale linearly and large values mostly buy memory pressure — 2–4 is a sensible ceiling. If the model itself refuses overlapping work, it surfaces as a retryable `Error::Generation` containing "already responding".

## Error handling

```rust
use fm_bridge::Error;

match bridge.complete(request).await {
    Ok(response) => println!("{}", response.text),
    Err(Error::ModelUnavailable(why)) => eprintln!("Apple Intelligence is off: {why}"),
    Err(Error::GuardrailViolation(_)) => eprintln!("blocked by safety filters"),
    Err(Error::ContextExceeded(_)) => eprintln!("prompt too long — trim the history"),
    Err(Error::InvalidSchema(why)) => eprintln!("bad schema: {why}"),
    Err(error) if error.is_retryable() => eprintln!("try again shortly: {error}"),
    Err(error) => eprintln!("{error}"),
}
```

Malformed requests and invalid schemas are caught in Rust *before* a process is spawned.

> **Note on token counts.** Apple exposes no tokenizer or token-count API to third-party code. Every `Usage` figure is **estimated** from character length (~4 chars/token). Use it for rough budgeting only, never for billing.

## Architecture

```
┌─────────────┐   JSON request (1 line, stdin)   ┌──────────────────┐
│  Rust crate │ ───────────────────────────────► │  Bridge     │
│             │                                  │  (Swift binary)  │
│  tokio      │ ◄─────────────────────────────── │                  │
└─────────────┘   NDJSON events (stdout)         └────────┬─────────┘
                                                          │
                                                 ┌────────▼─────────┐
                                                 │ FoundationModels │
                                                 │  (on-device)     │
                                                 └──────────────────┘
```

One process per request, torn down when it finishes — or immediately, if you drop the future or stream. Concurrent calls are fully independent, so `Bridge` is cheap to clone and share.

### Wire protocol

Request (one JSON object, one line):

```json
{"messages":[{"role":"user","content":"hi"}],"stream":true,"temperature":0.7,"maxTokens":256,"topK":40,"seed":42,"greedy":false,"schema":null,"streamStructured":false}
```

Response events, one JSON object per line:

| Event | Meaning |
|---|---|
| `{"delta":"..."}` | New text characters |
| `{"snapshot":{...}}` | Partially-filled structured object |
| `{"structured":{...}}` | Final structured object |
| `{"done":true,"usage":{...}}` | Generation finished |
| `{"error":"...","code":"...","reason":"..."}` | Failure (`reason` only on `model_unavailable`) |
| `{"ready":{"available":true,"contextSize":N,"supportedLanguages":[...]}}` | Reply to `--probe` |

Error codes: `model_unavailable`, `bad_request`, `schema_invalid`, `guardrail_violation`, `context_exceeded`, `generation_failed`, `unsupported_locale`, `concurrent_requests`. Unknown codes degrade to `Error::Generation`, so the helper can add new ones without breaking older crates.

Unavailability reasons: `device_not_eligible`, `not_enabled`, `model_not_ready`, `os_too_old`. These map onto [`Unavailable`]; an unrecognised token becomes `Unavailable::Unknown` rather than being mistaken for a known cause.

[`Unavailable`]: https://docs.rs/fm-bridge/latest/fm_bridge/enum.Unavailable.html

Exit codes: `0` success, `1` generation error, `2` malformed request, `3` model unavailable (`--probe`).

## Examples

```bash
cargo run --example stream_text       # token-by-token generation
cargo run --example structured_json   # constrained JSON output
cargo run --example chat              # multi-turn REPL
cargo run --example concurrent        # several requests at once
cargo run --example concurrent -- 1   # ...the same work, serially
```

The `concurrent` example reports the peak number of simultaneous requests and the wall-clock time, so you can watch the limit take effect: raising it from `1` to `4` should cut the total time roughly fourfold.

## Testing

Apple Intelligence cannot run in CI — it needs real Apple Silicon with the model downloaded. The test suite therefore drives the complete IPC path (spawn, stdin write, NDJSON parse, exit-status handling) against mock helpers in [`scripts/`](scripts/) that reproduce the Swift binary's stdout byte-for-byte, including crashes, stray `objc[...]` chatter on stdout, unknown event kinds, and every error code.

```bash
cargo test                                    # runs everywhere
swift build -c release --package-path swift   # macOS 26+ only
```

Verify against real hardware with the examples above.

## Shipping in a production app

End users have no Swift toolchain, so the helper is built once by you and embedded in your `.app`. No Swift runtime needs to ship with it: the Swift ABI has been stable since macOS 10.14.4, so `libswiftCore` and friends come from the OS, and `FoundationModels` is a system framework. The helper is a small self-contained executable.

### One binary covers every Apple silicon Mac

`arm64` is a single architecture across M1 through M4 — the generational differences are microarchitectural, not ABI. One `arm64` slice runs on all of them.

Do **not** bother with a universal binary. Apple Intelligence is Apple-silicon-only, so an `x86_64` slice could never reach an available model; on an Intel Mac the helper would exit with `model_unavailable` either way. Build `arm64` only and gate the feature at runtime instead:

```rust
if bridge.check_availability().await.is_ok() {
    // show the AI features
}
```

Set the deployment floor in `swift/Package.swift` (already `.macOS("26.0")`) and keep your app's own minimum at or above the OS versions where you enable these features.

### Build and sign the helper

```bash
SIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)" \
BUNDLE_ID="com.yourcompany.YourApp.FMBridge" \
./scripts/build-helper.sh
```

This writes a signed `dist/FMBridge` and prints its architectures, minimum OS, and linked frameworks. Pass `SANDBOX=1` if your app is sandboxed — the helper then gets `com.apple.security.inherit`, without which a sandboxed parent cannot launch it. Run with no environment set for a quick ad-hoc-signed local build.

Two rules that cause most shipping failures:

- **Nested code must be signed before the outer app is signed.** `codesign` does not recurse into `Contents/MacOS`, so a helper that is copied in unsigned stays unsigned, and the app's signature seals that defect in. Sign inside-out.
- **The helper needs the hardened runtime** (`--options runtime`) or notarization rejects the app.

Notarization itself only happens once, on the outer archive: `notarytool` unpacks nested containers, so a correctly pre-signed helper is covered by the app's submission.

### Embed it in the bundle

In Xcode, add a **Copy Files** build phase to your app target, set Destination to **Executables** (which is `Contents/MacOS`), drag in `dist/FMBridge`, and tick **Code Sign On Copy**. The result:

```text
YourApp.app/
└── Contents/
    ├── MacOS/
    │   ├── YourApp
    │   └── FMBridge
    └── Resources/
```

Verify before you ship — this catches an unsigned or wrongly-signed helper while you can still fix it:

```bash
codesign --verify --deep --strict --verbose=2 YourApp.app
spctl --assess --type execute -vv YourApp.app
```

### Point the crate at it

Use `Bridge::bundled()`, not `discover()` or `from_env()`:

```rust
let bridge = Bridge::bundled()?;
```

It resolves the helper strictly relative to the running executable — `Contents/MacOS/FMBridge`, then `Contents/Resources/FMBridge` — and canonicalizes the result. `discover()` and `from_env()` consult `PATH`, the working directory, and `$FM_BRIDGE_BIN`; those are conveniences for development, and none of them belong in a shipped app, where they let whoever launches your process choose which binary you execute. Keep `discover()` for your own `cargo run` workflow and `bundled()` for release builds:

```rust
let bridge = if cfg!(debug_assertions) {
    Bridge::discover()?
} else {
    Bridge::bundled()?
};
```

### Sanity-check on a clean machine

The failure mode you cannot see on your dev Mac is a missing or untrusted signature, because your machine already trusts your own certificates. Test on a Mac that has never seen your developer account, after downloading the app through a browser so it carries the quarantine bit.

## Troubleshooting

### `ld: warning: search path '/Library/Developer/CommandLineTools/Developer/...' not found`

Harmless — it is a *warning*, and the build still succeeds. SwiftPM passes
`-L$(DEVELOPER_DIR)/usr/lib` and `-F$(DEVELOPER_DIR)/Library/Frameworks` to the
linker; those directories exist inside `Xcode.app` but not inside the Command
Line Tools. The link resolves through the SDK regardless and the resulting
binary is identical.

It means `xcode-select` is pointing at the CLT rather than Xcode:

```bash
xcode-select -p    # prints /Library/Developer/CommandLineTools if so
```

`scripts/build-helper.sh` handles this automatically — when the active
toolchain is the CLT it selects the newest installed Xcode that carries a
macOS 26 SDK and reports what it picked. To fix it globally instead:

```bash
sudo xcode-select -s /Applications/Xcode.app/Contents/Developer
```

Or scope it to a single build, if you keep CLT selected on purpose:

```bash
env DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer \
  swift build -c release --package-path swift
```

Worth noting: the same misconfiguration is what causes `no such module
'FoundationModels'` below. Here it is only cosmetic because the SDK still
resolves, but pointing at Xcode fixes both.

### `error: no such module 'FoundationModels'`

The selected Xcode is too old. `FoundationModels` ships in the **macOS 26 SDK**
(Xcode 26+); an Xcode 16 toolchain has no such module, and the macOS 15 SDK will
not gain one. Note this is about the *SDK you build against*, not the macOS
version you are building on.

Check what is selected and what SDK it provides:

```bash
xcodebuild -version
xcrun --sdk macosx --show-sdk-version    # must be 26.x
```

If a newer Xcode is installed but not selected, switch to it:

```bash
sudo xcode-select -s /Applications/Xcode.app/Contents/Developer
```

Several Xcode versions can coexist, and the default is often *not* the newest —
this is the usual cause on CI runners, where the image ships Xcode 26 alongside
an older default. To pick a toolchain for one build without changing the global
default, set `DEVELOPER_DIR`:

```bash
DEVELOPER_DIR=/Applications/Xcode_26.5.app/Contents/Developer \
  swift build -c release --package-path swift
```

`scripts/build-helper.sh` does this for you: if the active toolchain is the
Command Line Tools, it scans `/Applications/Xcode*.app`, picks the newest one
providing a `MacOSX26*.sdk`, and exports `DEVELOPER_DIR` for the build.

### `warning: 'init(sampling:...)' is deprecated` / `error: incorrect argument label`

Apple renamed the `GenerationOptions` initializer's first parameter during the
macOS 26 cycle, so which spelling is correct depends on your SDK:

| Initializer | Availability |
| --- | --- |
| `init(samplingMode:temperature:maximumResponseTokens:)` | macOS 26.0+, `@backDeployed(before: macOS 27.0)`; missing from early 26.x SDKs |
| `init(sampling:temperature:maximumResponseTokens:)` | Present in every 26.x SDK; **deprecated** in recent ones |

The helper calls `samplingMode:`. Because it is `@backDeployed`, the compiler
emits the implementation into the binary, so it still runs on macOS 26 — the
newer label costs nothing at runtime.

If you build against an early 26.x SDK from before the rename, you will get
`error: incorrect argument label in call (have 'samplingMode:', expected
'sampling:')`. Change the one call in
`swift/Sources/FMBridge/FMBridgeMain.swift` back to `sampling:`; the two
initializers are otherwise identical. That spelling compiles everywhere but
emits a deprecation warning on current SDKs.

If you hit this with a different symbol, check your SDK version and compare it
against the "iOS 26.x+ / macOS 26.x+" line on the API's documentation page:

```bash
xcrun --sdk macosx --show-sdk-version
```

### `syntax error near unexpected token ';;'` when running a script

macOS ships **bash 3.2** and always has: bash 4 relicensed to GPLv3, which
Apple does not ship. Any shell script you run on a Mac -- including
`scripts/build-helper.sh` -- is therefore parsed by a 2006 shell, and three
constructs bite in particular:

| Construct | Problem |
| --- | --- |
| `declare -A` | Associative arrays are bash 4. |
| `case` inside `$( ... )` | bash 3.2 matches parens naively while scanning a command substitution, so the `)` closing a case pattern is read as the end of the substitution. The parse then fails on the following `;;`. |
| `sort -V` | GNU-only. BSD sort gained it later and availability varies; use `sort -t. -k1,1n -k2,2n`. |
| `mapfile` / `readarray`, `${v,,}`, `${v^^}` | All bash 4. |

The trap is that **modern bash and dash both accept the `case`-in-`$( )`
form**, so linting with your local shell -- or with dash, which is otherwise
stricter -- proves nothing. The only reliable check is a real bash 3.2:

```bash
# Scan for the known-bad constructs.
./scripts/check-shell-portability.sh

# Authoritative: parse everything with a real bash 3.2.
BASH32=/path/to/bash-3.2/bash ./scripts/check-shell-portability.sh
```

The guard scans every script in `scripts/`, so this class of breakage is
caught before it reaches a Mac.

### `Error::ModelUnavailable`

Run the probe to see the specific reason:

```bash
.build/release/FMBridge --probe
```

The `reason` field says which case you hit, and it is also surfaced in Rust as
`Error::ModelUnavailable { reason, .. }`:

| `reason` | `Unavailable` | Meaning |
|---|---|---|
| `device_not_eligible` | `DeviceNotEligible` | Intel hardware or unsupported silicon — permanent |
| `not_enabled` | `NotEnabled` | Switched off in System Settings — the user can fix it |
| `model_not_ready` | `ModelNotReady` | Assets still downloading — wait and retry |
| `os_too_old` | `OsTooOld` | Host predates macOS 26 — permanent |

`Error::is_retryable()` returns `true` only for `model_not_ready`; the other
causes will not resolve on their own, so retrying them just burns time.

### `Error::BinaryNotFound`

In development, set `FM_BRIDGE_BIN` to the built helper, or run from the
repository root so `Bridge::discover()` can find
`swift/.build/release/FMBridge`.

From a packaged app it means the Copy Files phase did not run or used the wrong
destination. Check what actually shipped:

```bash
ls -l YourApp.app/Contents/MacOS/
```

### The helper dies immediately in a packaged app

Usually a code-signing problem rather than a bug in the helper. Run it directly
to see the real reason:

```bash
YourApp.app/Contents/MacOS/FMBridge --probe
codesign -dv --verbose=4 YourApp.app/Contents/MacOS/FMBridge
```

`killed by signal 9` with no output is the signature check failing — the helper
is unsigned, or it was signed after the app was, which invalidates the app's
seal. Rebuild with `scripts/build-helper.sh` and re-sign inside-out.

If your app is sandboxed and the helper exits instantly, it is missing
`com.apple.security.inherit`; rebuild with `SANDBOX=1`.

## License

MIT OR Apache-2.0
