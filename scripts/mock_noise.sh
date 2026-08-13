#!/usr/bin/env bash
# Emits framework chatter and unknown event kinds alongside real events, to
# prove the parser ignores anything it does not recognise.
set -euo pipefail
read -r _request || true

printf '%s\n' 'objc[1234]: some framework warning on stdout'
printf '%s\n' ''
printf '%s\n' '{"ready":{"model":"on-device","contextSize":4096}}'
printf '%s\n' '{"delta":"real "}'
printf '%s\n' '{"someFutureEvent":{"a":1}}'
printf '%s\n' '{"delta":"text"}'
printf '%s\n' '{"done":true,"usage":{"promptTokens":2,"completionTokens":2}}'
