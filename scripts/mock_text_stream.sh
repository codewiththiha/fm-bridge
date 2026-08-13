#!/usr/bin/env bash
# Mimics FMBridge streaming a plain-text response.
# Reads (and discards) one JSON request line, then emits NDJSON deltas.
set -euo pipefail
read -r _request || true

printf '%s\n' '{"delta":"Hello"}'
printf '%s\n' '{"delta":", "}'
printf '%s\n' '{"delta":"world"}'
printf '%s\n' '{"delta":"!"}'
printf '%s\n' '{"done":true,"usage":{"promptTokens":7,"completionTokens":4}}'
