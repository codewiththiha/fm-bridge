#!/usr/bin/env bash
# Answers after MOCK_HOLD_MS (default 150), so overlap is measurable.
set -euo pipefail
read -r _request || true

hold_ms="${MOCK_HOLD_MS:-150}"

# bash 3.2 has no float arithmetic.
seconds=$(printf '%d.%03d' $((hold_ms / 1000)) $((hold_ms % 1000)))
sleep "$seconds"

printf '%s\n' '{"delta":"done"}'
printf '%s\n' '{"done":true,"usage":{"promptTokens":1,"completionTokens":1}}'
