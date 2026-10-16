#!/usr/bin/env bash
# Answers with the request line it received.
set -euo pipefail
read -r request || true

python3 - "$request" <<'PY'
import json, sys
payload = json.dumps({"delta": sys.argv[1]})
print(payload, flush=True)
print(json.dumps({"done": True, "usage": {"promptTokens": 1, "completionTokens": 1}}), flush=True)
PY
