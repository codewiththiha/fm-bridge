#!/usr/bin/env bash
# Mimics FMBridge returning a constrained-decoding JSON object.
set -euo pipefail
read -r _request || true

printf '%s\n' '{"structured":{"title":"Cacio e Pepe","minutes":15,"ingredients":["pasta","pecorino","black pepper"]}}'
printf '%s\n' '{"done":true,"usage":{"promptTokens":42,"completionTokens":18}}'
