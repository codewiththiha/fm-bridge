#!/usr/bin/env bash
# Mimics `FMBridge --probe`. Set MOCK_UNAVAILABLE to a reason token
# (device_not_eligible | not_enabled | model_not_ready) to simulate a Mac that
# cannot serve requests; "1" is accepted as a legacy alias for not_enabled.
set -euo pipefail

reason="${MOCK_UNAVAILABLE:-}"
case "$reason" in
  "" | 0)
    printf '%s\n' '{"ready":{"available":true,"contextSize":4096,"supportedLanguages":["en-US","es-ES"]}}'
    exit 0
    ;;
  1 | not_enabled)
    message='Apple Intelligence is not enabled; turn it on in System Settings.'
    reason=not_enabled
    ;;
  device_not_eligible)
    message='this device does not support Apple Intelligence (requires Apple silicon)'
    ;;
  model_not_ready)
    message='the on-device model is still downloading or preparing; try again shortly'
    ;;
  *)
    message='the on-device model is unavailable right now'
    ;;
esac

printf '{"error":"%s","code":"model_unavailable","reason":"%s"}\n' "$message" "$reason"
exit 3
