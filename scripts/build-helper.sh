#!/usr/bin/env bash
# Builds and signs a release helper: build-helper.sh [out-dir]
# README "Shipping" lists its environment.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="${1:-$REPO_ROOT/dist}"
SIGN_IDENTITY="${SIGN_IDENTITY:--}"
BUNDLE_ID="${BUNDLE_ID:-com.example.FMBridge}"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "error: the FMBridge helper can only be built on macOS." >&2
  exit 1
fi

# FoundationModels needs a macOS 26 SDK: recent Command Line Tools or Xcode.
sdk_major="$(xcrun --sdk macosx --show-sdk-version 2>/dev/null | cut -d. -f1 || true)"
case "$sdk_major" in ''|*[!0-9]*) sdk_major=0 ;; esac
if [[ -z "${DEVELOPER_DIR:-}" && "$sdk_major" -lt 26 ]]; then
  active="$(xcode-select -p 2>/dev/null || true)"
  if [[ "$active" != *"/Xcode"*".app/Contents/Developer" ]]; then
    best_dev=""
    best_ver=""
    for app in /Applications/Xcode.app /Applications/Xcode*.app; do
      dev="$app/Contents/Developer"
      [[ -d "$dev" ]] || continue
      for sdk in "$dev"/Platforms/MacOSX.platform/Developer/SDKs/MacOSX26*.sdk; do
        [[ -d "$sdk" ]] || continue
        ver="${sdk##*/MacOSX}"
        ver="${ver%.sdk}"
        if [[ -z "$best_ver" ]]; then
          best_ver="$ver"
          best_dev="$dev"
        else
          newest="$(printf '%s\n%s\n' "$best_ver" "$ver" | sort -t. -k1,1n -k2,2n | tail -1)"
          if [[ "$newest" == "$ver" && "$newest" != "$best_ver" ]]; then
            best_ver="$ver"
            best_dev="$dev"
          fi
        fi
      done
    done

    if [[ -n "$best_dev" ]]; then
      export DEVELOPER_DIR="$best_dev"
      echo "==> Using Xcode toolchain (macOS $best_ver SDK): $DEVELOPER_DIR"
      echo "    The active toolchain was '${active:-none}', which cannot build FoundationModels."
      echo "    Make it permanent with: sudo xcode-select -s $best_dev"
    else
      echo "warning: no Xcode with a macOS 26 SDK found; FoundationModels will not resolve." >&2
      echo "         Install Xcode 26, then: sudo xcode-select -s /Applications/Xcode.app/Contents/Developer" >&2
    fi
  fi
fi

echo "==> Building FMBridge (release, arm64)"
swift build -c release --package-path "$REPO_ROOT/swift" --arch arm64

BUILT_BIN="$(swift build -c release --package-path "$REPO_ROOT/swift" --arch arm64 --show-bin-path)/FMBridge"
[[ -f "$BUILT_BIN" ]] || { echo "error: build did not produce $BUILT_BIN" >&2; exit 1; }

mkdir -p "$OUT_DIR"
cp -f "$BUILT_BIN" "$OUT_DIR/FMBridge"
TARGET="$OUT_DIR/FMBridge"

# A sandboxed app can't launch a helper that doesn't inherit the sandbox.
CODESIGN_ARGS=(
  --sign "$SIGN_IDENTITY"
  --identifier "$BUNDLE_ID"
  --options runtime            # hardened runtime: required for notarization
  --force
)

# An ad-hoc signature cannot carry a secure timestamp.
if [[ "$SIGN_IDENTITY" != "-" ]]; then
  CODESIGN_ARGS+=(--timestamp)
fi

if [[ "${SANDBOX:-0}" == "1" ]]; then
  ENTITLEMENTS="$(mktemp -t fmbridge-entitlements).plist"
  cat > "$ENTITLEMENTS" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>com.apple.security.app-sandbox</key>
    <true/>
    <key>com.apple.security.inherit</key>
    <true/>
</dict>
</plist>
PLIST
  CODESIGN_ARGS+=(--entitlements "$ENTITLEMENTS")
  echo "==> Signing with sandbox inheritance"
fi

echo "==> Signing as '$BUNDLE_ID' with identity '$SIGN_IDENTITY'"
codesign "${CODESIGN_ARGS[@]}" "$TARGET"

echo "==> Verifying"
codesign --verify --strict --verbose=2 "$TARGET"
echo
echo "Architectures: $(lipo -archs "$TARGET")"
echo "Minimum OS:    $(vtool -show-build-version "$TARGET" 2>/dev/null | awk '/minos/ {print $2; exit}')"
echo "Linked frameworks:"
otool -L "$TARGET" | sed -n '2,$p' | grep -Ei 'foundationmodels|swift' | sed 's/^/  /' || true
echo
echo "Built: $TARGET"

if [[ "$SIGN_IDENTITY" == "-" ]]; then
  cat >&2 <<'WARN'

WARNING: this binary is ad-hoc signed and will be blocked by Gatekeeper on any
other Mac. For distribution, rebuild with a Developer ID:

  SIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)" \
  BUNDLE_ID="com.yourcompany.YourApp.FMBridge" \
  ./scripts/build-helper.sh
WARN
fi
