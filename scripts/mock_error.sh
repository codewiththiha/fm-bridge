#!/usr/bin/env bash
# Mimics FMBridge failing with a typed error code.
# Override the code/message with MOCK_ERROR_CODE / MOCK_ERROR_MESSAGE.
set -euo pipefail
read -r _request || true

code="${MOCK_ERROR_CODE:-guardrail_violation}"
message="${MOCK_ERROR_MESSAGE:-The request was blocked by safety guardrails.}"
printf '{"error":"%s","code":"%s"}\n' "$message" "$code"
exit 1
