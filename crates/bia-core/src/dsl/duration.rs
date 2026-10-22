//! Duration strings such as `500ms`, `30s`, `5m`, `2h` and `3d`.

use std::borrow::Cow;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::str::FromStr;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A duration written as a whole number followed by a unit: `ms`, `s`, `m`, `h` or `d`.
///
/// It keeps the unit it was written in, so serialising gives back the same string.
/// Two Durations are equal when they span the same time (`60s == 1m`).
#[derive(Debug, Clone, Copy)]
pub struct Duration {
    amount: u64,
    unit: DurationUnit,
}

/// The unit a [`Duration`] was written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurationUnit {
    Milliseconds,
    Seconds,
    Minutes,
    Hours,
    Days,
}

impl DurationUnit {
    fn suffix(self) -> &'static str {
        match self {
            Self::Milliseconds => "ms",
            Self::Seconds => "s",
            Self::Minutes => "m",
            Self::Hours => "h",
            Self::Days => "d",
        }
    }

    fn millis(self) -> u64 {
        match self {
            Self::Milliseconds => 1,
            Self::Seconds => 1_000,
            Self::Minutes => 60_000,
            Self::Hours => 3_600_000,
            Self::Days => 86_400_000,
        }
    }
}

impl Duration {
    /// Builds a Duration, or `None` when it does not fit in `u64` milliseconds.
    pub fn new(amount: u64, unit: DurationUnit) -> Option<Self> {
        amount.checked_mul(unit.millis())?;
        Some(Self { amount, unit })
    }

    pub fn amount(self) -> u64 {
        self.amount
    }

    pub fn unit(self) -> DurationUnit {
        self.unit
    }

    /// The span in milliseconds.
    pub fn as_millis(self) -> u64 {
        // `new` and `from_str` reject amounts that overflow.
        self.amount * self.unit.millis()
    }

    /// The span as a [`std::time::Duration`].
    pub fn to_std(self) -> std::time::Duration {
        std::time::Duration::from_millis(self.as_millis())
    }
}

impl PartialEq for Duration {
    fn eq(&self, other: &Self) -> bool {
        self.as_millis() == other.as_millis()
    }
}

impl Eq for Duration {}

impl Hash for Duration {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_millis().hash(state);
    }
}

impl PartialOrd for Duration {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Duration {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.as_millis().cmp(&other.as_millis())
    }
}

impl fmt::Display for Duration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.amount, self.unit.suffix())
    }
}

/// A string that is not a valid [`Duration`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "invalid duration `{0}`: expected a whole number followed by `ms`, `s`, `m`, `h` or `d` (e.g. `30s`)"
)]
pub struct InvalidDuration(String);

impl FromStr for Duration {
    type Err = InvalidDuration;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let invalid = || InvalidDuration(s.to_owned());
        let digits_end = s.find(|c: char| !c.is_ascii_digit()).ok_or_else(invalid)?;
        let (digits, suffix) = s.split_at(digits_end);
        // A leading zero is only allowed on `0` itself, so each Duration has one spelling.
        if digits.is_empty() || (digits.len() > 1 && digits.starts_with('0')) {
            return Err(invalid());
        }
        let unit = match suffix {
            "ms" => DurationUnit::Milliseconds,
            "s" => DurationUnit::Seconds,
            "m" => DurationUnit::Minutes,
            "h" => DurationUnit::Hours,
            "d" => DurationUnit::Days,
            _ => return Err(invalid()),
        };
        let amount = digits.parse().map_err(|_| invalid())?;
        Self::new(amount, unit).ok_or_else(invalid)
    }
}

impl Serialize for Duration {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Duration {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for Duration {
    fn schema_name() -> Cow<'static, str> {
        "Duration".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "A whole number followed by a unit: `ms`, `s`, `m`, `h` or `d` (e.g. `500ms`, `30s`, `3d`).",
            "type": "string",
            "pattern": "^(0|[1-9][0-9]*)(ms|s|m|h|d)$",
        })
    }
}
