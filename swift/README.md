# FMBridge (Swift helper)

The Swift half of [`fm-bridge`](../README.md). It links `FoundationModels`,
reads one JSON request line from stdin, and writes NDJSON events to stdout.

```bash
swift build -c release
.build/release/FMBridge --probe          # availability check
echo '{"messages":[{"role":"user","content":"hi"}]}' | .build/release/FMBridge
```

Requires macOS 26+ on Apple silicon with Apple Intelligence enabled.

| File | Purpose |
|---|---|
| `Protocol.swift` | Wire types, NDJSON emitter, error-code taxonomy |
| `SchemaBuilder.swift` | `PropertyConfig` → `DynamicGenerationSchema` translation |
| `FMBridgeMain.swift` | Entry point: prompt assembly, three generation modes, error mapping |

Exit codes: `0` success, `1` generation error, `2` malformed request, `3` model unavailable.
