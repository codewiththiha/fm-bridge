#!/usr/bin/env bash
# Echoes the request it received back as the response text, so tests can assert
# on the exact JSON the Rust crate put on the wire.
set -euo pipefail
read -r request || true

python3 - "$request" <<'PY'
import json, sys
payload = json.dumps({"delta": sys.argv[1]})
print(payload, flush=True)
print(json.dumps({"done": True, "usage": {"promptTokens": 1, "completionTokens": 1}}), flush=True)
PY
