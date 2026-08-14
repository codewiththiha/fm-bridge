import Foundation

// MARK: - Wire Protocol (must stay in sync with the Rust crate's `types.rs`)

/// A single chat message. `role` is one of "system", "user", "assistant".
struct BridgeMessage: Codable, Sendable {
    let role: String
    let content: String
}

/// Recursive description of a JSON schema node, mirroring Rust's `SchemaProperty`.
struct PropertyConfig: Codable, Sendable {
    let name: String
    let description: String?
    /// "string" | "integer" | "number" | "boolean" | "object" | "array"
    let type: String
    /// Present when `type == "object"`.
    let properties: [PropertyConfig]?
    /// Present when `type == "array"`.
    let items: IndirectProperty?
    /// Numeric bounds `[min, max]` for "integer"/"number".
    let range: [Double]?
    /// String enumeration constraint.
    let anyOf: [String]?
    /// Regex-like pattern for "string".
    let pattern: String?
    /// Element-count bounds `[min, max]` for "array". Either entry may be null.
    let count: [Int?]?
    /// Defaults to `false` (required) when omitted.
    let optional: Bool?
}

/// `PropertyConfig` cannot directly contain itself in a stored property, so array
/// element schemas are wrapped in a reference box.
final class IndirectProperty: Codable, Sendable {
    let value: PropertyConfig

    init(value: PropertyConfig) { self.value = value }

    init(from decoder: Decoder) throws {
        self.value = try PropertyConfig(from: decoder)
    }

    func encode(to encoder: Encoder) throws {
        try value.encode(to: encoder)
    }
}

struct SchemaConfig: Codable, Sendable {
    let name: String
    let description: String?
    let properties: [PropertyConfig]
}

struct BridgeRequest: Codable, Sendable {
    let messages: [BridgeMessage]
    let stream: Bool?
    let temperature: Double?
    let maxTokens: Int?
    let topK: Int?
    let seed: UInt64?
    let greedy: Bool?
    let schema: SchemaConfig?
    /// Emit `{"snapshot": ...}` events while a structured response streams.
    let streamStructured: Bool?
}

// MARK: - Outbound events

enum Outbound {
    /// Writes one NDJSON line to stdout and flushes immediately so the Rust
    /// side sees each event as it happens rather than at process exit.
    static func emit(_ object: [String: Any]) {
        guard JSONSerialization.isValidJSONObject(object),
              let data = try? JSONSerialization.data(withJSONObject: object, options: [.withoutEscapingSlashes]),
              var line = String(data: data, encoding: .utf8)
        else { return }
        line.append("\n")
        FileHandle.standardOutput.write(Data(line.utf8))
    }

    static func delta(_ text: String) {
        emit(["delta": text])
    }

    static func structured(_ value: Any) {
        emit(["structured": value])
    }

    static func snapshot(_ value: Any) {
        emit(["snapshot": value])
    }

    static func done(promptTokens: Int, completionTokens: Int) {
        emit([
            "done": true,
            "usage": [
                "promptTokens": promptTokens,
                "completionTokens": completionTokens,
            ],
        ])
    }

    static func error(_ message: String, code: String, reason: String? = nil) {
        var payload: [String: Any] = ["error": message, "code": code]
        if let reason {
            payload["reason"] = reason
        }
        emit(payload)
    }

    static func ready(_ info: [String: Any]) {
        emit(["ready": info])
    }
}

/// Errors the bridge reports with a stable machine-readable `code` so the Rust
/// side can map them onto typed variants instead of string matching.
struct BridgeError: Error {
    let code: String
    let message: String
    /// Only set for `model_unavailable`, where the cause is actionable.
    var reason: String?

    static func unavailable(_ message: String, reason: String? = nil) -> BridgeError {
        BridgeError(code: "model_unavailable", message: message, reason: reason)
    }
    static func badRequest(_ message: String) -> BridgeError {
        BridgeError(code: "bad_request", message: message)
    }
    static func schemaInvalid(_ message: String) -> BridgeError {
        BridgeError(code: "schema_invalid", message: message)
    }
    static func guardrail(_ message: String) -> BridgeError {
        BridgeError(code: "guardrail_violation", message: message)
    }
    static func contextExceeded(_ message: String) -> BridgeError {
        BridgeError(code: "context_exceeded", message: message)
    }
    static func generation(_ message: String) -> BridgeError {
        BridgeError(code: "generation_failed", message: message)
    }
}
