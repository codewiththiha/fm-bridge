import Foundation
import FoundationModels

/// Translates the wire `PropertyConfig` tree into Apple's `DynamicGenerationSchema`.
///
/// Notes on the real API surface (verified against the macOS 26 SDK):
///   * `DynamicGenerationSchema.Property.init(name:description:schema:isOptional:)`
///     — the parameter is `isOptional`, *not* `optionality:`.
///   * `DynamicGenerationSchema.init(type:guides:)` is generic over a `Generable`
///     value type and takes `[GenerationGuide<Value>]`.
///   * `GenerationSchema.init(root:dependencies:)` **throws**.
enum SchemaBuilder {

    static func buildRoot(from config: SchemaConfig) throws -> GenerationSchema {
        guard !config.properties.isEmpty else {
            throw BridgeError.schemaInvalid("schema '\(config.name)' declares no properties")
        }

        let rootProperties = try config.properties.map { try buildProperty(from: $0) }
        let root = DynamicGenerationSchema(
            name: config.name,
            description: config.description,
            properties: rootProperties
        )

        do {
            return try GenerationSchema(root: root, dependencies: [])
        } catch {
            throw BridgeError.schemaInvalid("could not validate schema: \(error.localizedDescription)")
        }
    }

    static func buildProperty(from prop: PropertyConfig) throws -> DynamicGenerationSchema.Property {
        DynamicGenerationSchema.Property(
            name: prop.name,
            description: prop.description,
            schema: try buildSchema(from: prop),
            isOptional: prop.optional ?? false
        )
    }

    static func buildSchema(from prop: PropertyConfig) throws -> DynamicGenerationSchema {
        switch prop.type.lowercased() {

        case "object":
            guard let nested = prop.properties, !nested.isEmpty else {
                throw BridgeError.schemaInvalid("object property '\(prop.name)' has no nested properties")
            }
            return DynamicGenerationSchema(
                name: uniqueName(for: prop),
                description: prop.description,
                properties: try nested.map { try buildProperty(from: $0) }
            )

        case "array":
            guard let items = prop.items else {
                throw BridgeError.schemaInvalid("array property '\(prop.name)' is missing an `items` schema")
            }
            let (minimum, maximum) = arrayBounds(prop.count)
            return DynamicGenerationSchema(
                arrayOf: try buildSchema(from: items.value),
                minimumElements: minimum,
                maximumElements: maximum
            )

        case "string":
            var guides: [GenerationGuide<String>] = []
            // `DynamicGenerationSchema.init(name:description:anyOf:)` unions
            // *schemas*, not strings; a string enumeration is expressed as the
            // `GenerationGuide<String>.anyOf([String])` guide instead.
            if let choices = prop.anyOf, !choices.isEmpty {
                guides.append(.anyOf(choices))
            }
            if let pattern = prop.pattern {
                // `GenerationGuide.pattern` needs a real `Regex`; an unparseable
                // pattern is a caller error, so surface it rather than silently drop it.
                do {
                    guides.append(.pattern(try Regex(pattern)))
                } catch {
                    throw BridgeError.schemaInvalid(
                        "property '\(prop.name)' has an invalid regex pattern: \(error.localizedDescription)"
                    )
                }
            }
            return DynamicGenerationSchema(type: String.self, guides: guides)

        case "integer", "int":
            var guides: [GenerationGuide<Int>] = []
            if let bounds = prop.range, bounds.count == 2 {
                let lower = Int(bounds[0].rounded())
                let upper = Int(bounds[1].rounded())
                guard lower <= upper else {
                    throw BridgeError.schemaInvalid("property '\(prop.name)' has an inverted range")
                }
                guides.append(.range(lower...upper))
            }
            return DynamicGenerationSchema(type: Int.self, guides: guides)

        case "number", "double", "float":
            var guides: [GenerationGuide<Double>] = []
            if let bounds = prop.range, bounds.count == 2 {
                guard bounds[0] <= bounds[1] else {
                    throw BridgeError.schemaInvalid("property '\(prop.name)' has an inverted range")
                }
                guides.append(.range(bounds[0]...bounds[1]))
            }
            return DynamicGenerationSchema(type: Double.self, guides: guides)

        case "boolean", "bool":
            return DynamicGenerationSchema(type: Bool.self, guides: [])

        default:
            throw BridgeError.schemaInvalid(
                "property '\(prop.name)' has unsupported type '\(prop.type)'"
            )
        }
    }

    private static func arrayBounds(_ count: [Int?]?) -> (Int?, Int?) {
        guard let count else { return (nil, nil) }
        let minimum = count.count > 0 ? count[0] : nil
        let maximum = count.count > 1 ? count[1] : nil
        return (minimum, maximum)
    }

    /// Nested object schemas need names that are unique within one request;
    /// property names are unique per level, so qualify them to avoid collisions.
    private static func uniqueName(for prop: PropertyConfig) -> String {
        let sanitized = prop.name
            .unicodeScalars
            .map { CharacterSet.alphanumerics.contains($0) ? Character($0) : "_" }
            .reduce(into: "") { $0.append($1) }
        return sanitized.isEmpty ? "Value" : sanitized
    }
}
