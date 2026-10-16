#!/usr/bin/env bash
# Rejects what macOS's bash 3.2 can't parse.
# BASH32=/path/to/bash-3.2 parses every file with it too.
set -uo pipefail

cd "$(dirname "$0")/.."

status=0
note() { printf '  %s\n' "$1"; }
fail() { printf 'FAIL %s\n' "$1"; status=1; }

workdir=$(mktemp -d)
trap 'rm -rf "$workdir"' EXIT

# Expand the workflow's run: blocks into standalone scripts.
if command -v python3 > /dev/null 2>&1; then
  python3 - "$workdir" <<'PY' || status=1
import re, sys, os
try:
    import yaml
except ImportError:
    sys.exit(0)
workdir = sys.argv[1]
path = ".github/workflows/ci.yml"
if not os.path.exists(path):
    sys.exit(0)
doc = yaml.safe_load(open(path))
i = 0
for job, cfg in (doc.get("jobs") or {}).items():
    for step in cfg.get("steps") or []:
        run = step.get("run")
        if not run:
            continue
        i += 1
        # Substitute GitHub expressions the way the runner does.
        body = re.sub(r"\$\{\{[^}]*\}\}", "placeholder", run)
        name = re.sub(r"[^A-Za-z0-9]+", "_", f"{job}_{step.get('name','step')}")
        with open(os.path.join(workdir, f"{i:02d}_{name}.sh"), "w") as fh:
            fh.write(body)
PY
else
  note "python3 not found; skipping workflow extraction"
fi

targets=$(ls "$workdir"/*.sh 2>/dev/null; ls scripts/*.sh 2>/dev/null)

echo "Scanning for bash-3.2-incompatible constructs..."
for file in $targets; do
  label=${file#"$workdir"/}

  # Skip this file: it necessarily contains the patterns it searches for.
  case "$label" in
    *check-shell-portability.sh) continue ;;
  esac

  # Strip comments so prose describing these constructs is not flagged.
  code=$(sed 's/[[:space:]]*#.*$//' "$file")

  if echo "$code" | grep -qE 'declare[[:space:]]+-A|mapfile|readarray|\$\{[A-Za-z_][A-Za-z0-9_]*(,,|\^\^)'; then
    fail "$label: bash 4 only construct (declare -A / mapfile / case modification)"
  fi

  # bash 3.2 ends $( ) at a case pattern's ')' and dies on ';;'.
  if echo "$code" | awk '
      {
        line = $0
        opens = gsub(/\$\(/, "&", line)
        closes = gsub(/\)/, "&", line)
        # Same-line $( ) pairs close themselves.
        if (depth > 0 && line ~ /(^|[;[:space:]])case[[:space:]]/) bad = 1
        depth += opens - closes
        if (depth < 0) depth = 0
      }
      END { exit(bad ? 1 : 0) }
    '; then
    :
  else
    fail "$label: 'case' inside \$( ) - bash 3.2 misparses this"
  fi

  if echo "$code" | grep -qE '(^|[|[:space:]])sort([[:space:]]+-[a-zA-Z.,0-9]+)*[[:space:]]+-[a-zA-Z]*V'; then
    fail "$label: 'sort -V' is GNU only; use 'sort -t. -k1,1n -k2,2n'"
  fi
done
[ "$status" -eq 0 ] && note "no incompatible constructs found"

if [ -n "${BASH32:-}" ] && [ -x "${BASH32:-}" ]; then
  echo "Parsing with $("$BASH32" --version | head -1)..."
  for file in $targets; do
    label=${file#"$workdir"/}
    if err=$("$BASH32" -n "$file" 2>&1); then
      note "OK   $label"
    else
      fail "$label: $err"
    fi
  done
else
  echo "Set BASH32=/path/to/bash-3.2/bash for an authoritative parse check."
fi

exit "$status"
