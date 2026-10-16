import Foundation
import FoundationModels

/// The wire `PropertyConfig` tree as a `DynamicGenerationSchema`.
enum SchemaBuilder {

    static func buildRoot(from config: SchemaConfig) throws -> GenerationSchema {
        guard !config.properties.isEmpty else {
            throw BridgeError.schemaInvalid("schema '\(config.name)' declares no properties")
        }

        let rootProperties = try config.properties.map {
            try buildProperty(from: $0, path: config.name)
        }
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

    static func buildProperty(
        from prop: PropertyConfig, path: String
    ) throws -> DynamicGenerationSchema.Property {
        DynamicGenerationSchema.Property(
            name: prop.name,
            description: prop.description,
            schema: try buildSchema(from: prop, path: path + "_" + prop.name),
            isOptional: prop.optional ?? false
        )
    }

    /// `path` names nested objects uniquely across the whole schema.
    static func buildSchema(
        from prop: PropertyConfig, path: String
    ) throws -> DynamicGenerationSchema {
        switch prop.type.lowercased() {

        case "object":
            guard let nested = prop.properties, !nested.isEmpty else {
                throw BridgeError.schemaInvalid("object property '\(prop.name)' has no nested properties")
            }
            return DynamicGenerationSchema(
                name: sanitized(path),
                description: prop.description,
                properties: try nested.map { try buildProperty(from: $0, path: path) }
            )

        case "array":
            guard let items = prop.items else {
                throw BridgeError.schemaInvalid("array property '\(prop.name)' is missing an `items` schema")
            }
            let (minimum, maximum) = arrayBounds(prop.count)
            return DynamicGenerationSchema(
                arrayOf: try buildSchema(from: items.value, path: path + "_item"),
                minimumElements: minimum,
                maximumElements: maximum
            )

        case "string":
            var guides: [GenerationGuide<String>] = []
            // `init(name:description:anyOf:)` unions schemas, not strings.
            if let choices = prop.anyOf, !choices.isEmpty {
                guides.append(.anyOf(choices))
            }
            if let pattern = prop.pattern {
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

    private static func sanitized(_ path: String) -> String {
        let sanitized = path
            .unicodeScalars
            .map { CharacterSet.alphanumerics.contains($0) ? Character($0) : "_" }
            .reduce(into: "") { $0.append($1) }
        return sanitized.isEmpty ? "Value" : sanitized
    }
}
