#!/usr/bin/env bash
# Streams a plain-text answer.
set -euo pipefail
read -r _request || true

printf '%s\n' '{"delta":"Hello"}'
printf '%s\n' '{"delta":", "}'
printf '%s\n' '{"delta":"world"}'
printf '%s\n' '{"delta":"!"}'
printf '%s\n' '{"done":true,"usage":{"promptTokens":7,"completionTokens":4}}'
