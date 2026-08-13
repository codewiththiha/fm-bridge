#!/usr/bin/env bash
# Mimics FMBridge dying before it finishes the response.
set -euo pipefail
read -r _request || true

printf '%s\n' '{"delta":"partial"}'
echo "dyld: Library not loaded: FoundationModels.framework" >&2
exit 9
