import Foundation
import FoundationModels

// MARK: - Prompt assembly

/// Splits the message list into system instructions and a flattened prompt.
/// Consecutive `system` messages are joined; everything else becomes transcript.
func buildPrompt(from messages: [BridgeMessage]) -> (instructions: String?, prompt: String) {
    var systemParts: [String] = []
    var turns: [String] = []

    for message in messages {
        switch message.role.lowercased() {
        case "system":
            systemParts.append(message.content)
        case "assistant":
            turns.append("Assistant: \(message.content)")
        default:
            turns.append("User: \(message.content)")
        }
    }

    // A single trailing user message is by far the common case; sending it bare
    // (no "User:" prefix) matches how the model was trained and measurably
    // improves output quality.
    let prompt: String
    if turns.count == 1, let only = turns.first, only.hasPrefix("User: ") {
        prompt = String(only.dropFirst("User: ".count))
    } else {
        prompt = turns.joined(separator: "\n")
    }

    let instructions = systemParts.isEmpty ? nil : systemParts.joined(separator: "\n\n")
    return (instructions, prompt)
}

/// Rough token estimate (~4 chars/token). The framework does not expose real
/// token counts to third-party callers, so this is explicitly an approximation
/// and is documented as such on the Rust side.
func estimateTokens(_ text: String) -> Int {
    max(1, text.count / 4)
}

func makeOptions(from request: BridgeRequest) -> GenerationOptions {
    var samplingMode: GenerationOptions.SamplingMode? = nil
    if request.greedy == true {
        samplingMode = .greedy
    } else if let topK = request.topK {
        samplingMode = .random(top: topK, seed: request.seed)
    }

    // Apple renamed this parameter during the macOS 26 cycle: `sampling:` is
    // deprecated in favour of `samplingMode:`.
    //
    // `samplingMode:` is `@backDeployed(before: macOS 27.0)`, so the compiler
    // emits its implementation into this binary and it runs correctly on
    // macOS 26 — there is no runtime availability cost to preferring it.
    //
    // If you are building against an early 26.x SDK from before the rename and
    // this line fails with "incorrect argument label", change `samplingMode:`
    // back to `sampling:`; the two are otherwise identical.
    return GenerationOptions(
        samplingMode: samplingMode,
        temperature: request.temperature,
        maximumResponseTokens: request.maxTokens
    )
}

/// Maps framework errors onto stable wire codes.
func classify(_ error: Error) -> BridgeError {
    if let bridgeError = error as? BridgeError { return bridgeError }

    if let generationError = error as? LanguageModelSession.GenerationError {
        switch generationError {
        case .exceededContextWindowSize:
            return .contextExceeded("prompt and response exceed the session context window")
        case .guardrailViolation:
            return .guardrail("the request or response was blocked by safety guardrails")
        case .unsupportedLanguageOrLocale:
            return BridgeError(code: "unsupported_locale",
                               message: "the model cannot respond in this language or locale")
        case .concurrentRequests:
            return BridgeError(code: "concurrent_requests",
                               message: "the session is already responding to another request")
        default:
            return .generation(generationError.localizedDescription)
        }
    }

    return .generation(error.localizedDescription)
}

func availabilityMessage(_ reason: SystemLanguageModel.Availability.UnavailableReason) -> String {
    switch reason {
    case .deviceNotEligible:
        return "this device does not support Apple Intelligence (requires Apple silicon)"
    case .appleIntelligenceNotEnabled:
        return "Apple Intelligence is not enabled; turn it on in System Settings"
    case .modelNotReady:
        return "the on-device model is still downloading or preparing; try again shortly"
    @unknown default:
        return "the on-device model is unavailable right now"
    }
}

/// Stable machine-readable token for an unavailability reason.
///
/// Sent alongside the prose message so the Rust side can branch on the cause
/// without matching on wording that may change between OS releases.
func availabilityReason(_ reason: SystemLanguageModel.Availability.UnavailableReason) -> String {
    switch reason {
    case .deviceNotEligible:
        return "device_not_eligible"
    case .appleIntelligenceNotEnabled:
        return "not_enabled"
    case .modelNotReady:
        return "model_not_ready"
    @unknown default:
        return "unknown"
    }
}

/// Parses `GeneratedContent.jsonString` into a `JSONSerialization` object graph
/// so it can be re-emitted as a nested value instead of an escaped string.
func jsonObject(from content: GeneratedContent) -> Any {
    let raw = content.jsonString
    guard let data = raw.data(using: .utf8),
          let object = try? JSONSerialization.jsonObject(
              with: data,
              options: [.fragmentsAllowed]
          )
    else {
        return raw
    }
    return object
}

// MARK: - Request handling

func handle(_ request: BridgeRequest) async throws {
    let model = SystemLanguageModel.default

    switch model.availability {
    case .available:
        break
    case .unavailable(let reason):
        throw BridgeError.unavailable(
            availabilityMessage(reason),
            reason: availabilityReason(reason)
        )
    @unknown default:
        throw BridgeError.unavailable(
            "the on-device model is unavailable right now",
            reason: "unknown"
        )
    }

    let (instructionText, promptText) = buildPrompt(from: request.messages)
    guard !promptText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
        throw BridgeError.badRequest("request contains no user or assistant message")
    }

    // `Instructions` and `Prompt` are both built from runtime strings here
    // rather than the @-builder DSL, which only accepts literals.
    let session = instructionText.map { LanguageModelSession(instructions: Instructions($0)) }
        ?? LanguageModelSession()

    let prompt = Prompt(promptText)
    let options = makeOptions(from: request)
    let promptTokens = estimateTokens((instructionText ?? "") + promptText)
    let shouldStream = request.stream ?? true

    if let schemaConfig = request.schema {
        // ── Structured generation ────────────────────────────────────────────
        // Contrary to a common misconception, guided generation *does* stream:
        // `streamResponse(to:schema:)` yields `ResponseStream<GeneratedContent>`
        // snapshots of the partially-filled object.
        let schema = try SchemaBuilder.buildRoot(from: schemaConfig)

        if shouldStream && (request.streamStructured ?? false) {
            let stream = session.streamResponse(to: prompt, schema: schema, options: options)
            // Snapshots are cumulative, so the last one *is* the final object.
            // Iterating consumes the stream, so we must not also call
            // `collect()` here — we keep the last snapshot instead.
            var latest: GeneratedContent?
            for try await snapshot in stream {
                latest = snapshot.content
                Outbound.snapshot(jsonObject(from: snapshot.content))
            }
            guard let content = latest else {
                throw BridgeError.generation("the model produced no structured output")
            }
            Outbound.structured(jsonObject(from: content))
            Outbound.done(
                promptTokens: promptTokens,
                completionTokens: estimateTokens(content.jsonString)
            )
        } else {
            let response = try await session.respond(to: prompt, schema: schema, options: options)
            let content = response.content
            Outbound.structured(jsonObject(from: content))
            Outbound.done(
                promptTokens: promptTokens,
                completionTokens: estimateTokens(content.jsonString)
            )
        }
        return
    }

    if shouldStream {
        // ── Streaming text ───────────────────────────────────────────────────
        // Snapshots are *cumulative*, so deltas are derived by diffing against
        // what we already emitted. Diffing on Character (not UTF-16 offsets)
        // keeps grapheme clusters and emoji intact.
        let stream = session.streamResponse(to: prompt, options: options)
        var emitted = ""
        for try await snapshot in stream {
            let text = snapshot.content
            guard text.count > emitted.count else {
                // A snapshot can rewrite earlier text; fall back to a full resend.
                if text != emitted, !text.isEmpty {
                    emitted = text
                    Outbound.delta(text)
                }
                continue
            }
            let delta = String(text.dropFirst(emitted.count))
            emitted = text
            if !delta.isEmpty { Outbound.delta(delta) }
        }
        Outbound.done(promptTokens: promptTokens, completionTokens: estimateTokens(emitted))
    } else {
        // ── Non-streaming text ───────────────────────────────────────────────
        let response = try await session.respond(to: prompt, options: options)
        Outbound.delta(response.content)
        Outbound.done(
            promptTokens: promptTokens,
            completionTokens: estimateTokens(response.content)
        )
    }
}

// MARK: - Entry point

/// Reads one NDJSON request from stdin, serves it, and exits. One process per
/// request keeps the memory model trivial and guarantees the model resources
/// are released; the Rust side sets `kill_on_drop` so an abandoned stream
/// cannot leak a daemon.
@main
struct FMBridgeMain {
    static func main() async {
        // `--probe` lets callers check availability without spending a generation.
        if CommandLine.arguments.contains("--probe") {
            let model = SystemLanguageModel.default
            switch model.availability {
            case .available:
                // Report what the model can actually do, so callers can size
                // prompts and check locale support without a trial generation.
                let languages = model.supportedLanguages
                    .map(\.maximalIdentifier)
                    .sorted()
                Outbound.ready([
                    "available": true,
                    "contextSize": model.contextSize,
                    "supportedLanguages": languages,
                ])
                exit(0)
            case .unavailable(let reason):
                Outbound.error(
                    availabilityMessage(reason),
                    code: "model_unavailable",
                    reason: availabilityReason(reason)
                )
                exit(3)
            @unknown default:
                Outbound.error(
                    "unknown availability state",
                    code: "model_unavailable",
                    reason: "unknown"
                )
                exit(3)
            }
        }

        guard let line = readLine(strippingNewline: true),
              let data = line.data(using: .utf8)
        else {
            Outbound.error("no request received on stdin", code: "bad_request")
            exit(2)
        }

        let request: BridgeRequest
        do {
            request = try JSONDecoder().decode(BridgeRequest.self, from: data)
        } catch {
            Outbound.error("malformed request JSON: \(error)", code: "bad_request")
            exit(2)
        }

        do {
            try await handle(request)
            exit(0)
        } catch {
            let bridgeError = classify(error)
            FileHandle.standardError.write(
                Data("FMBridge: \(bridgeError.code): \(bridgeError.message)\n".utf8)
            )
            Outbound.error(
                bridgeError.message,
                code: bridgeError.code,
                reason: bridgeError.reason
            )
            exit(1)
        }
    }
}
