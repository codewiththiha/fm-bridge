#!/usr/bin/env bash
# Real events mixed with chatter and unknown kinds.
set -euo pipefail
read -r _request || true

printf '%s\n' 'objc[1234]: some framework warning on stdout'
printf '%s\n' ''
printf '%s\n' '{"ready":{"model":"on-device","contextSize":4096}}'
printf '%s\n' '{"delta":"real "}'
printf '%s\n' '{"someFutureEvent":{"a":1}}'
printf '%s\n' '{"delta":"text"}'
printf '%s\n' '{"done":true,"usage":{"promptTokens":2,"completionTokens":2}}'
