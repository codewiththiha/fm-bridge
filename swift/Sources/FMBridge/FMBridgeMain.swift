import Foundation
import FoundationModels

// MARK: - Prompt assembly

/// System messages become instructions; the rest a flat transcript.
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

    // A lone user turn goes bare: the model answers it better.
    let prompt: String
    if turns.count == 1, let only = turns.first, only.hasPrefix("User: ") {
        prompt = String(only.dropFirst("User: ".count))
    } else {
        prompt = turns.joined(separator: "\n")
    }

    let instructions = systemParts.isEmpty ? nil : systemParts.joined(separator: "\n\n")
    return (instructions, prompt)
}

/// About four characters a token; the framework exposes no tokenizer.
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

    // `sampling:` builds on every 26.x SDK; `samplingMode:` only on newer.
    return GenerationOptions(
        sampling: samplingMode,
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
        case .rateLimited:
            return BridgeError(code: "rate_limited",
                               message: "the system is rate limiting on-device generation")
        case .assetsUnavailable:
            return .unavailable("the model assets are unavailable right now",
                                reason: "model_not_ready")
        case .refusal:
            return BridgeError(code: "refusal", message: "the model declined to answer")
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

/// A stable token Rust branches on; the message may be reworded.
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

/// Nested JSON rather than an escaped string.
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

    let session = instructionText.map { LanguageModelSession(instructions: Instructions($0)) }
        ?? LanguageModelSession()

    let prompt = Prompt(promptText)
    let options = makeOptions(from: request)
    let promptTokens = estimateTokens((instructionText ?? "") + promptText)
    let shouldStream = request.stream ?? true

    if let schemaConfig = request.schema {
        let schema = try SchemaBuilder.buildRoot(from: schemaConfig)

        if shouldStream && (request.streamStructured ?? false) {
            let stream = session.streamResponse(to: prompt, schema: schema, options: options)
            // Snapshots are cumulative: the last one is the answer.
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
        // Cumulative snapshots, diffed by Character to keep graphemes whole.
        let stream = session.streamResponse(to: prompt, options: options)
        var emitted = ""
        for try await snapshot in stream {
            let text = snapshot.content
            guard text.count > emitted.count else {
                // A rewrite of earlier text is resent whole.
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
        let response = try await session.respond(to: prompt, options: options)
        Outbound.delta(response.content)
        Outbound.done(
            promptTokens: promptTokens,
            completionTokens: estimateTokens(response.content)
        )
    }
}

// MARK: - Entry point

/// Serves one NDJSON request from stdin, then exits.
@main
struct FMBridgeMain {
    static func main() async {
        if CommandLine.arguments.contains("--probe") {
            let model = SystemLanguageModel.default
            switch model.availability {
            case .available:
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
