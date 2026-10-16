# AGENTS.md

Rules for anyone changing this repository, human or agent. Read it before the
first edit; it is short on purpose.

## Project

`fm-bridge` lets Rust call Apple's on-device Foundation Models. Apple ships the
framework for Swift only, so the crate has two halves that talk NDJSON over
stdin and stdout, one helper process per request:

| Path | What lives there |
|---|---|
| `src/bridge.rs` | `Bridge`: locating, concurrency, timeouts, `complete`, `stream`, probe |
| `src/session.rs` | One helper process: spawn, write, read lines, reap |
| `src/wire.rs` | The protocol on the Rust side |
| `src/request.rs`, `src/response.rs` | Public request and answer types |
| `src/schema.rs` | `Schema`, its builder, and `Schema::from_json` |
| `src/error.rs` | `Error`, `Unavailable`, retryability |
| `src/locate.rs` | `from_env`, `bundled`, `discover` |
| `swift/Sources/FMBridge/` | The helper: `Protocol.swift` mirrors `src/wire.rs` |
| `scripts/mock_*.sh` | Fake helpers the tests run instead of Swift |

The crate is a building block. It knows nothing about any app's prompts,
settings or other providers; keep it that way.

## The protocol has two owners

`src/wire.rs` and `swift/Sources/FMBridge/Protocol.swift` describe one
protocol. Change both in the same commit, and add or update the mock that
exercises the change. A new error code needs, together:

1. the Swift side emitting it,
2. a mapping in `Error::from_code` (a new variant only if callers act on it
   differently),
3. a decision in `Error::is_retryable`,
4. a test through `scripts/mock_error.sh`.

Unknown codes and reasons must keep degrading (`Error::Generation`,
`Unavailable::Unknown`), so an older crate survives a newer helper.

## Never crash

The helper's output, environment variables and JSON schemas are untrusted.

- No `unwrap`, `expect`, `panic!` or indexing in library code; clippy warns and
  CI denies warnings. Tests may use them.
- No `unsafe`; the crate forbids it.
- Recursion over input (schemas, `$ref`s) is bounded by `MAX_DEPTH`.
- Buffers fed by the helper are capped (`STDERR_CAP`).
- A dropped future or stream must kill its helper and free its slot. Tests
  prove both; keep them.

## Swift

Nothing on Linux can compile the helper. The `swift` CI job on `macos-26`
builds it and runs `--probe`; treat a red `swift` job like a failed compile.
Keep `@unknown default` in every switch over framework enums: Apple adds cases.

## Comments

A comment carries a constraint the code cannot show. It never narrates the
change, argues for a design, restates the next line or records history; those
go in the commit message or `README.md`.

`python3 tools/comment_check.py FILE...` measures it: three lines, eighty
characters a line, fifteen words a block, doc comments included. Run it on
the files you touched; CI runs it on every tracked file. A block that truly
needs more says `comment-check: allow` on the line above it.

## Commits

- Conventional commits: `type(scope): summary`, imperative, lower case, no
  trailing period, subject at most 50 characters. Types: `feat`, `fix`, `perf`,
  `refactor`, `docs`, `test`, `build`, `ci`, `chore`. Scopes: `bridge`,
  `schema`, `wire`, `swift`, `scripts`, `tools`.
- A body only when the why isn't obvious: a few short lines, wrapped at 72.
  No test counts, file lists or progress reports.
- `python3 tools/commit_check.py --install` arms a hook that refuses long
  subjects; `--range main..HEAD` audits a branch.
- Every commit builds and passes tests alone.
- Never commit credentials.

## Gates

CI runs all of these on every push; run what you can before pushing:

```text
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
./scripts/check-shell-portability.sh
python3 tools/comment_check.py
python3 tools/commit_check.py --range HEAD
swift build -c release --package-path swift    # macOS 26 only
```

Never weaken an assertion or edit `.github/workflows/` to turn a check green.

## Definition of done

1. The change is implemented on both sides of the protocol if it touches it.
2. A test fails without the change and passes with it.
3. CI is green for the pushed commit, `swift` job included.
4. `README.md` describes the new behaviour.
