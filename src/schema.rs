use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::{Error, Result};

/// Deepest nesting a schema may have; guards untrusted JSON.
pub const MAX_DEPTH: usize = 16;

/// The JSON type of a [`SchemaProperty`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SchemaType {
    /// A string.
    String,
    /// A signed integer.
    Integer,
    /// A double.
    Number,
    /// `true` or `false`.
    Boolean,
    /// Nested fields in [`SchemaProperty::properties`].
    Object,
    /// A list of [`SchemaProperty::items`].
    Array,
}

/// One field of a [`Schema`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SchemaProperty {
    /// Field name in the generated JSON.
    pub name: String,
    /// Guidance the model reads; the main steering lever.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The field's type.
    #[serde(rename = "type")]
    pub property_type: SchemaType,
    /// Fields of an object.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<Vec<SchemaProperty>>,
    /// Element shape of an array.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<Box<SchemaProperty>>,
    /// Inclusive bounds of a number.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub range: Option<[f64; 2]>,
    /// The only strings allowed.
    #[serde(rename = "anyOf", skip_serializing_if = "Option::is_none")]
    pub any_of: Option<Vec<String>>,
    /// A regex the string must match.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    /// Element count bounds of an array.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count: Option<[Option<usize>; 2]>,
    /// Whether the model may leave the field out.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub optional: bool,
}

impl SchemaProperty {
    fn bare(name: impl Into<String>, property_type: SchemaType) -> Self {
        Self {
            name: name.into(),
            description: None,
            property_type,
            properties: None,
            items: None,
            range: None,
            any_of: None,
            pattern: None,
            count: None,
            optional: false,
        }
    }

    /// A string field.
    pub fn string(name: impl Into<String>) -> Self {
        Self::bare(name, SchemaType::String)
    }

    /// An integer field.
    pub fn integer(name: impl Into<String>) -> Self {
        Self::bare(name, SchemaType::Integer)
    }

    /// A number field.
    pub fn number(name: impl Into<String>) -> Self {
        Self::bare(name, SchemaType::Number)
    }

    /// A boolean field.
    pub fn boolean(name: impl Into<String>) -> Self {
        Self::bare(name, SchemaType::Boolean)
    }

    /// An object field.
    pub fn object(name: impl Into<String>, properties: Vec<SchemaProperty>) -> Self {
        Self {
            properties: Some(properties),
            ..Self::bare(name, SchemaType::Object)
        }
    }

    /// An array field of `items`.
    pub fn array(name: impl Into<String>, items: SchemaProperty) -> Self {
        Self {
            items: Some(Box::new(items)),
            ..Self::bare(name, SchemaType::Array)
        }
    }

    /// Adds guidance for the model.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Bounds a number, inclusive.
    pub fn range(mut self, min: f64, max: f64) -> Self {
        self.range = Some([min, max]);
        self
    }

    /// Limits a string to `choices`.
    pub fn any_of<I, S>(mut self, choices: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.any_of = Some(choices.into_iter().map(Into::into).collect());
        self
    }

    /// Makes a string match `pattern`.
    pub fn pattern(mut self, pattern: impl Into<String>) -> Self {
        self.pattern = Some(pattern.into());
        self
    }

    /// Bounds an array's length; either end may be open.
    pub fn count(mut self, min: impl Into<Option<usize>>, max: impl Into<Option<usize>>) -> Self {
        self.count = Some([min.into(), max.into()]);
        self
    }

    /// Lets the model leave the field out.
    pub fn optional(mut self) -> Self {
        self.optional = true;
        self
    }

    fn validate(&self, depth: usize) -> std::result::Result<(), String> {
        let name = &self.name;
        if depth > MAX_DEPTH {
            return Err(format!("'{name}' nests deeper than {MAX_DEPTH}"));
        }
        if name.trim().is_empty() {
            return Err("a property has an empty name".into());
        }
        match self.property_type {
            SchemaType::Object => {
                let fields = self.properties.as_deref().unwrap_or_default();
                check_fields(name, fields, depth)?;
            }
            SchemaType::Array => {
                let items = self
                    .items
                    .as_ref()
                    .ok_or(format!("array '{name}' has no items"))?;
                items.validate(depth + 1)?;
                if let Some([Some(min), Some(max)]) = self.count
                    && min > max
                {
                    return Err(format!("array '{name}' has min count above max"));
                }
            }
            SchemaType::Integer | SchemaType::Number => {
                if let Some([min, max]) = self.range
                    && !(min.is_finite() && max.is_finite() && min <= max)
                {
                    return Err(format!("'{name}' has an invalid range"));
                }
            }
            SchemaType::String => {
                if self.any_of.as_ref().is_some_and(Vec::is_empty) {
                    return Err(format!("'{name}' allows no strings"));
                }
            }
            SchemaType::Boolean => {}
        }
        Ok(())
    }
}

fn check_fields(
    owner: &str,
    fields: &[SchemaProperty],
    depth: usize,
) -> std::result::Result<(), String> {
    if fields.is_empty() {
        return Err(format!("'{owner}' declares no properties"));
    }
    for (i, field) in fields.iter().enumerate() {
        if fields.iter().take(i).any(|f| f.name == field.name) {
            return Err(format!("'{owner}' repeats property '{}'", field.name));
        }
        field.validate(depth + 1)?;
    }
    Ok(())
}

/// The object shape constrained decoding must produce.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Schema {
    /// Name of the generated type.
    pub name: String,
    /// Guidance for the object as a whole.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The fields.
    pub properties: Vec<SchemaProperty>,
}

impl Schema {
    /// A schema named `name` with `properties`.
    pub fn new(name: impl Into<String>, properties: Vec<SchemaProperty>) -> Self {
        Self {
            name: name.into(),
            description: None,
            properties,
        }
    }

    /// Adds guidance for the whole object.
    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Reads a standard JSON Schema; README lists the keywords read.
    pub fn from_json(name: impl Into<String>, schema: &Value) -> Result<Self> {
        let reader = JsonReader { root: schema };
        let root = reader.resolve(schema, 0)?;
        let properties = reader.fields(root, 0)?;
        let description = text(root, "description");
        let schema = Self {
            name: name.into(),
            description,
            properties,
        };
        schema.validate().map_err(Error::InvalidSchema)?;
        Ok(schema)
    }

    pub(crate) fn validate(&self) -> std::result::Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("schema name must not be empty".into());
        }
        check_fields(&self.name, &self.properties, 0)
    }
}

fn text(node: &Value, key: &str) -> Option<String> {
    node.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn number(node: &Value, key: &str) -> Option<f64> {
    node.get(key).and_then(Value::as_f64)
}

fn count(node: &Value, key: &str) -> Option<usize> {
    node.get(key)
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
}

struct JsonReader<'a> {
    root: &'a Value,
}

impl<'a> JsonReader<'a> {
    fn bad(message: impl Into<String>) -> Error {
        Error::InvalidSchema(message.into())
    }

    fn resolve(&self, node: &'a Value, depth: usize) -> Result<&'a Value> {
        let mut node = node;
        for _ in 0..=MAX_DEPTH.saturating_sub(depth) {
            let Some(path) = node.get("$ref").and_then(Value::as_str) else {
                return Ok(node);
            };
            let pointer = path
                .strip_prefix('#')
                .ok_or_else(|| Self::bad(format!("only local $ref is supported: {path}")))?;
            node = self
                .root
                .pointer(pointer)
                .ok_or_else(|| Self::bad(format!("unresolved $ref {path}")))?;
        }
        Err(Self::bad("$ref chain too deep"))
    }

    fn kind(node: &Value) -> Result<(SchemaType, bool)> {
        let names: Vec<&str> = match node.get("type") {
            Some(Value::String(one)) => vec![one.as_str()],
            Some(Value::Array(many)) => many.iter().filter_map(Value::as_str).collect(),
            _ if node.get("properties").is_some() => vec!["object"],
            _ if node.get("enum").is_some() => vec!["string"],
            _ => return Err(Self::bad("a property has no type")),
        };
        let nullable = names.contains(&"null");
        let mut real = names.into_iter().filter(|n| *n != "null");
        let kind = match (real.next(), real.next()) {
            (Some("string"), None) => SchemaType::String,
            (Some("integer"), None) => SchemaType::Integer,
            (Some("number"), None) => SchemaType::Number,
            (Some("boolean"), None) => SchemaType::Boolean,
            (Some("object"), None) => SchemaType::Object,
            (Some("array"), None) => SchemaType::Array,
            (Some(other), None) => return Err(Self::bad(format!("unsupported type {other}"))),
            _ => return Err(Self::bad("a property needs exactly one non-null type")),
        };
        Ok((kind, nullable))
    }

    fn fields(&self, node: &'a Value, depth: usize) -> Result<Vec<SchemaProperty>> {
        let empty = Map::new();
        let properties = node
            .get("properties")
            .and_then(Value::as_object)
            .unwrap_or(&empty);
        let required: Vec<&str> = node
            .get("required")
            .and_then(Value::as_array)
            .map(|r| r.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        properties
            .iter()
            .map(|(name, child)| {
                let mut field = self.property(name, child, depth + 1)?;
                field.optional |= !required.contains(&name.as_str());
                Ok(field)
            })
            .collect()
    }

    fn property(&self, name: &str, node: &'a Value, depth: usize) -> Result<SchemaProperty> {
        if depth > MAX_DEPTH {
            return Err(Self::bad(format!("'{name}' nests deeper than {MAX_DEPTH}")));
        }
        let node = self.resolve(node, depth)?;
        let (kind, nullable) = Self::kind(node)?;
        let mut field = SchemaProperty::bare(name, kind);
        field.description = text(node, "description");
        field.optional = nullable;
        match kind {
            SchemaType::Object => field.properties = Some(self.fields(node, depth)?),
            SchemaType::Array => {
                let items = node
                    .get("items")
                    .ok_or_else(|| Self::bad(format!("array '{name}' has no items")))?;
                field.items = Some(Box::new(self.property("item", items, depth + 1)?));
                let bounds = [count(node, "minItems"), count(node, "maxItems")];
                field.count = (bounds != [None, None]).then_some(bounds);
            }
            SchemaType::Integer | SchemaType::Number => {
                let min = number(node, "minimum");
                let max = number(node, "maximum");
                field.range = min.zip(max).map(|(lo, hi)| [lo, hi]);
            }
            SchemaType::String => {
                field.pattern = text(node, "pattern");
                field.any_of = node.get("enum").and_then(Value::as_array).map(|values| {
                    values
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                });
            }
            SchemaType::Boolean => {}
        }
        Ok(field)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_a_json_schema() {
        let schema = Schema::from_json(
            "Word",
            &json!({
                "type": "object",
                "description": "A word",
                "properties": {
                    "pos": {"type": "string", "enum": ["noun", "verb"]},
                    "meaning": {"type": "string", "description": "Plain words"},
                    "synonyms": {
                        "type": "array",
                        "items": {"type": "string"},
                        "minItems": 2, "maxItems": 5
                    },
                    "level": {"type": ["integer", "null"], "minimum": 1, "maximum": 6}
                },
                "required": ["pos", "meaning", "synonyms"]
            }),
        )
        .unwrap();
        let field = |n: &str| schema.properties.iter().find(|p| p.name == n).unwrap();
        assert_eq!(schema.description.as_deref(), Some("A word"));
        assert_eq!(field("pos").any_of.as_ref().unwrap().len(), 2);
        assert_eq!(field("synonyms").count, Some([Some(2), Some(5)]));
        assert_eq!(field("level").range, Some([1.0, 6.0]));
        assert!(field("level").optional && !field("meaning").optional);
    }

    #[test]
    fn follows_local_refs() {
        let schema = Schema::from_json(
            "Root",
            &json!({
                "$ref": "#/$defs/Root",
                "$defs": {
                    "Root": {"properties": {"inner": {"$ref": "#/$defs/Inner"}}},
                    "Inner": {"type": "object", "properties": {"x": {"type": "boolean"}}}
                }
            }),
        )
        .unwrap();
        let inner = &schema.properties[0];
        assert_eq!(inner.property_type, SchemaType::Object);
        assert_eq!(inner.properties.as_ref().unwrap()[0].name, "x");
    }

    #[test]
    fn rejects_cycles_and_bad_shapes() {
        let cyclic = json!({"$ref": "#/$defs/A", "$defs": {"A": {"$ref": "#/$defs/A"}}});
        let nested = json!({"$defs": {"N": {"type": "object", "properties": {
            "n": {"$ref": "#/$defs/N"}}}}, "$ref": "#/$defs/N"});
        let cases = [
            cyclic,
            nested,
            json!({"type": "object", "properties": {}}),
            json!({"properties": {"a": {"type": ["string", "integer"]}}}),
            json!({"properties": {"a": {"$ref": "https://x/y"}}}),
            json!({"properties": {"a": {"type": "array"}}}),
        ];
        for case in cases {
            assert!(Schema::from_json("S", &case).is_err(), "{case}");
        }
    }

    #[test]
    fn validation_catches_duplicates_and_inverted_bounds() {
        let duplicate = Schema::new(
            "S",
            vec![SchemaProperty::string("a"), SchemaProperty::string("a")],
        );
        let inverted = Schema::new(
            "S",
            vec![SchemaProperty::array("a", SchemaProperty::string("x")).count(3, 1)],
        );
        let no_choice = Schema::new(
            "S",
            vec![SchemaProperty::string("a").any_of(Vec::<String>::new())],
        );
        for schema in [duplicate, inverted, no_choice] {
            assert!(schema.validate().is_err());
        }
    }
}
