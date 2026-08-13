#!/usr/bin/env bash
# Streams slowly, so tests can exercise the request timeout.
set -euo pipefail
read -r _request || true

printf '%s\n' '{"delta":"tick"}'
sleep "${MOCK_SLEEP_SECONDS:-30}"
printf '%s\n' '{"done":true,"usage":{"promptTokens":1,"completionTokens":1}}'
