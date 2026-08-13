#!/usr/bin/env bash
# Mimics FMBridge streaming partial snapshots of a structured response.
set -euo pipefail
read -r _request || true

printf '%s\n' '{"snapshot":{"title":"Cacio"}}'
printf '%s\n' '{"snapshot":{"title":"Cacio e Pepe","minutes":15}}'
printf '%s\n' '{"structured":{"title":"Cacio e Pepe","minutes":15,"ingredients":["pasta","pecorino"]}}'
printf '%s\n' '{"done":true,"usage":{"promptTokens":42,"completionTokens":21}}'
