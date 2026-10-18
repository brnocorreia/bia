//! Identifiers and names that appear in Definitions, each checked on parse.

use std::borrow::Cow;
use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize};

/// Longest identifier or name accepted anywhere in the DSL.
const MAX_LEN: usize = 64;

/// `[a-z][a-z0-9_]*`
fn is_snake_ident(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some('a'..='z'))
        && chars.all(|c| matches!(c, 'a'..='z' | '0'..='9' | '_'))
}

/// `[a-z][a-z0-9_-]*`
fn is_slug(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some('a'..='z'))
        && chars.all(|c| matches!(c, 'a'..='z' | '0'..='9' | '_' | '-'))
}

/// `category.operation`, both parts `[a-z][a-z0-9_]*`.
fn is_action_name(s: &str) -> bool {
    s.split_once('.')
        .is_some_and(|(category, operation)| is_snake_ident(category) && is_snake_ident(operation))
}

macro_rules! name_type {
    (
        $description:literal
        $name:ident, check: $check:expr, pattern: $pattern:literal, expected: $expected:literal
    ) => {
        #[doc = $description]
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Checks `value` against the DSL's rules for this name.
            pub fn new(value: impl Into<String>) -> Result<Self, InvalidName> {
                let value = value.into();
                if value.len() <= MAX_LEN && $check(value.as_str()) {
                    Ok(Self(value))
                } else {
                    Err(InvalidName { value, expected: $expected })
                }
            }

            /// The name as written in the Definition.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
            }
        }

        impl JsonSchema for $name {
            fn schema_name() -> Cow<'static, str> {
                stringify!($name).into()
            }

            fn json_schema(_: &mut SchemaGenerator) -> Schema {
                json_schema!({
                    "description": $description,
                    "type": "string",
                    "pattern": $pattern,
                    "maxLength": MAX_LEN,
                })
            }
        }
    };
}

name_type! {
    "A Workflow's id, stable across its Versions (e.g. `onboarding-email`)."
    WorkflowId, check: is_slug, pattern: "^[a-z][a-z0-9_-]*$",
    expected: "lowercase letters, digits, `_` or `-`, starting with a letter"
}

name_type! {
    "A Step's id, unique across the whole Definition."
    StepId, check: is_snake_ident, pattern: "^[a-z][a-z0-9_]*$",
    expected: "lowercase letters, digits or `_`, starting with a letter"
}

name_type! {
    "A Trigger's id, unique within the Definition."
    TriggerId, check: is_snake_ident, pattern: "^[a-z][a-z0-9_]*$",
    expected: "lowercase letters, digits or `_`, starting with a letter"
}

name_type! {
    "An Action's name, `category.operation` (e.g. `http.request`)."
    ActionName, check: is_action_name, pattern: "^[a-z][a-z0-9_]*\\.[a-z][a-z0-9_]*$",
    expected: "`category.operation`, each part lowercase letters, digits or `_`, starting with a letter"
}

name_type! {
    "A Queue's name (e.g. `emails`)."
    QueueName, check: is_slug, pattern: "^[a-z][a-z0-9_-]*$",
    expected: "lowercase letters, digits, `_` or `-`, starting with a letter"
}

/// A string that breaks the rules for an identifier or name.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid name `{value}`: expected {expected}, at most 64 characters")]
pub struct InvalidName {
    value: String,
    expected: &'static str,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_ids_are_snake_case() {
        assert!(StepId::new("fetch_user2").is_ok());
        for bad in ["", "Fetch", "2fetch", "fetch-user", "_x", "fetch.user"] {
            assert!(StepId::new(bad).is_err(), "{bad:?} should be rejected");
        }
        assert!(StepId::new("a".repeat(65)).is_err());
    }

    #[test]
    fn slugs_allow_hyphens() {
        assert!(WorkflowId::new("onboarding-email").is_ok());
        assert!(QueueName::new("emails-dlq").is_ok());
        assert!(QueueName::new("-emails").is_err());
    }

    #[test]
    fn action_names_have_a_category_and_an_operation() {
        assert!(ActionName::new("http.request").is_ok());
        assert!(ActionName::new("flow.loop_until").is_ok());
        for bad in [
            "http",
            "http.",
            ".request",
            "http.request.get",
            "Http.request",
        ] {
            assert!(ActionName::new(bad).is_err(), "{bad:?} should be rejected");
        }
    }
}
