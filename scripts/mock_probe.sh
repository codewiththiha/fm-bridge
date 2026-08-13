#!/usr/bin/env bash
# Mimics `FMBridge --probe`. Set MOCK_UNAVAILABLE=1 to simulate a Mac with
# Apple Intelligence switched off.
set -euo pipefail

if [[ "${MOCK_UNAVAILABLE:-0}" == "1" ]]; then
  printf '%s\n' '{"error":"Apple Intelligence is not enabled in System Settings.","code":"model_unavailable"}'
  exit 3
fi

printf '%s\n' '{"ready":{"available":true,"contextSize":4096}}'
