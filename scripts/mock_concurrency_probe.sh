#!/usr/bin/env bash
# Mimics FMBridge taking a measurable amount of time to answer, so tests can
# observe whether requests overlap or are serialized by the concurrency limit.
#
# MOCK_HOLD_MS controls how long the "generation" takes (default 150 ms).
set -euo pipefail
read -r _request || true

hold_ms="${MOCK_HOLD_MS:-150}"

# `sleep` accepts fractional seconds on macOS and Linux; bash 3.2 has no
# floating-point arithmetic, so build the decimal string with printf.
seconds=$(printf '%d.%03d' $((hold_ms / 1000)) $((hold_ms % 1000)))
sleep "$seconds"

printf '%s\n' '{"delta":"done"}'
printf '%s\n' '{"done":true,"usage":{"promptTokens":1,"completionTokens":1}}'
