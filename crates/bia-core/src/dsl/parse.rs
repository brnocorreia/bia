//! Parsing Definitions and resources from JSON, YAML or a JSON value.
//!
//! YAML is first converted to JSON, so every format goes through the same typed
//! deserialisation and reports errors with a JSON Pointer into the document.

use serde::de::DeserializeOwned;
use serde_json::Value;
use serde_path_to_error::Segment;

use super::{Definition, Document, QueueResource, SchemaVersion};

/// A document that could not be parsed, with the place it went wrong.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}: {kind}", if path.is_empty() { "/" } else { path })]
pub struct ParseError {
    /// JSON Pointer (RFC 6901) to the offending value; empty for the document root.
    pub path: String,
    pub kind: ParseErrorKind,
}

/// Why a document could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ParseErrorKind {
    /// The text is not well-formed JSON or YAML.
    #[error("syntax error: {message}")]
    Syntax { message: String },
    /// A Definition without `schema_version`.
    #[error(
        "missing `schema_version`; this build supports {}",
        SchemaVersion::CURRENT
    )]
    MissingSchemaVersion,
    /// A Definition whose `schema_version` this build does not support.
    #[error(
        "unsupported `schema_version` {found}; this build supports {}",
        SchemaVersion::CURRENT
    )]
    UnsupportedSchemaVersion { found: String },
    /// A document whose `kind` names no known resource.
    #[error("unknown `kind` {found}; expected `queue`, or no `kind` for a Workflow Definition")]
    UnknownKind { found: String },
    /// Well-formed text that does not have the DSL's shape.
    #[error("{message}")]
    Shape { message: String },
}

impl ParseError {
    fn new(path: impl Into<String>, kind: ParseErrorKind) -> Self {
        Self {
            path: path.into(),
            kind,
        }
    }
}

/// Converts a YAML document to its canonical JSON value, as the CLI does before sending a
/// Definition to the server.
pub fn yaml_to_json(yaml: &str) -> Result<Value, ParseError> {
    let mut options = serde_saphyr::Options::default();
    // YAML 1.2: only `true` and `false` are booleans, so `on` or `no` stay strings.
    options.strict_booleans = true;
    options.with_snippet = false;
    serde_saphyr::from_str_with_options(yaml, options).map_err(|err| {
        ParseError::new(
            "",
            ParseErrorKind::Syntax {
                message: err.to_string(),
            },
        )
    })
}

fn json_to_value(json: &str) -> Result<Value, ParseError> {
    serde_json::from_str(json).map_err(|err| {
        ParseError::new(
            "",
            ParseErrorKind::Syntax {
                message: err.to_string(),
            },
        )
    })
}

/// Deserialises `value` into `T`, reporting where it failed as a JSON Pointer.
fn typed<T: DeserializeOwned>(value: Value) -> Result<T, ParseError> {
    serde_path_to_error::deserialize(value).map_err(|err| {
        let path = pointer(err.path());
        ParseError::new(
            path,
            ParseErrorKind::Shape {
                message: err.into_inner().to_string(),
            },
        )
    })
}

fn pointer(path: &serde_path_to_error::Path) -> String {
    let mut pointer = String::new();
    for segment in path.iter() {
        let token = match segment {
            Segment::Seq { index } => index.to_string(),
            Segment::Map { key } => key.replace('~', "~0").replace('/', "~1"),
            Segment::Enum { variant } => variant.replace('~', "~0").replace('/', "~1"),
            Segment::Unknown => continue,
        };
        pointer.push('/');
        pointer.push_str(&token);
    }
    pointer
}

fn check_schema_version(value: &Value) -> Result<(), ParseError> {
    // A non-object root is reported by the typed pass, with the expected shape.
    let Value::Object(map) = value else {
        return Ok(());
    };
    match map.get("schema_version") {
        None => Err(ParseError::new(
            "/schema_version",
            ParseErrorKind::MissingSchemaVersion,
        )),
        Some(version) if version.as_u64().and_then(SchemaVersion::new).is_some() => Ok(()),
        Some(version) => Err(ParseError::new(
            "/schema_version",
            ParseErrorKind::UnsupportedSchemaVersion {
                found: version.to_string(),
            },
        )),
    }
}

impl Definition {
    pub fn from_value(value: Value) -> Result<Self, ParseError> {
        check_schema_version(&value)?;
        typed(value)
    }

    pub fn from_json(json: &str) -> Result<Self, ParseError> {
        Self::from_value(json_to_value(json)?)
    }

    pub fn from_yaml(yaml: &str) -> Result<Self, ParseError> {
        Self::from_value(yaml_to_json(yaml)?)
    }
}

impl QueueResource {
    pub fn from_value(value: Value) -> Result<Self, ParseError> {
        typed(value)
    }

    pub fn from_json(json: &str) -> Result<Self, ParseError> {
        Self::from_value(json_to_value(json)?)
    }

    pub fn from_yaml(yaml: &str) -> Result<Self, ParseError> {
        Self::from_value(yaml_to_json(yaml)?)
    }
}

impl Document {
    /// Parses a document from its JSON value: a resource when it has a top-level `kind`,
    /// otherwise a Workflow Definition.
    pub fn from_value(value: Value) -> Result<Self, ParseError> {
        match value.get("kind") {
            None => Definition::from_value(value).map(Self::Workflow),
            Some(Value::String(kind)) if kind == "queue" => {
                QueueResource::from_value(value).map(Self::Queue)
            }
            Some(kind) => Err(ParseError::new(
                "/kind",
                ParseErrorKind::UnknownKind {
                    found: kind.to_string(),
                },
            )),
        }
    }

    pub fn from_json(json: &str) -> Result<Self, ParseError> {
        Self::from_value(json_to_value(json)?)
    }

    pub fn from_yaml(yaml: &str) -> Result<Self, ParseError> {
        Self::from_value(yaml_to_json(yaml)?)
    }
}
