use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Minimal JSON schema definition used for OpenRouter structured outputs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JsonSchemaDefinition {
    /// JSON Schema type (e.g. "object").
    #[serde(rename = "type")]
    pub schema_type: String,
    /// Property definitions keyed by name.
    pub properties: Map<String, Value>,
    /// Optional list of required property names.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub required: Option<Vec<String>>,
    /// Whether additional properties are permitted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub additional_properties: Option<bool>,
}

/// Configuration wrapper sent to the OpenRouter API for structured responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JsonSchemaConfig {
    /// Public schema name to include in requests.
    pub name: String,
    /// If true, responses must strictly follow the schema.
    pub strict: bool,
    /// The underlying JSON schema definition.
    pub schema: JsonSchemaDefinition,
}
