use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChoreClaim {
    /// This caller runs the Chore now.
    Claimed,
    /// The Chore already ran at this time, within its interval.
    RanAt(DateTime<Utc>),
}
