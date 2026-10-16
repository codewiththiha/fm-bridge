import Foundation

// MARK: - Wire protocol; mirrors the Rust crate's `src/wire.rs`

/// A single chat message. `role` is one of "system", "user", "assistant".
struct BridgeMessage: Codable, Sendable {
    let role: String
    let content: String
}

/// One schema node; mirrors Rust's `SchemaProperty`.
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

/// A box: a struct can't store itself.
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
    /// One line, written unbuffered so Rust sees it at once.
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

/// An error with a stable `code` Rust maps to a variant.
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
