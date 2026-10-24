//! Cron expressions read in a time zone, and the jitter added to their firings.
//!
//! Expressions have five fields (minute, hour, day of month, month, day of week) in the
//! Vixie-cron dialect, plus `L`, `W`, `#` and aliases such as `@daily`, parsed by `croner`.
//! Occurrences follow the wall clock of the time zone: across a DST change `0 9 * * *` stays at
//! 09:00 local, a time skipped by the change fires at the first instant after the gap, and a
//! time repeated by it fires once. Semantics: ADR-0040.

use chrono::{DateTime, TimeDelta, Utc};
use chrono_tz::Tz;
use croner::parser::{CronParser, Seconds, Year};
use croner::{Cron as Pattern, Direction};

use crate::dsl::Duration;

/// A cron expression and the time zone it is read in.
#[derive(Debug, Clone)]
pub struct Cron {
    pattern: Pattern,
    tz: Tz,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CronError {
    #[error("invalid cron expression `{expr}`: {reason}")]
    InvalidExpression { expr: String, reason: String },
    #[error("cron expression `{0}` never fires")]
    NeverFires(String),
    #[error("unknown time zone `{0}`: expected an IANA name such as `America/Sao_Paulo`")]
    UnknownTimeZone(String),
}

impl Cron {
    /// Reads `expr` in `tz`, an IANA time zone name, or in UTC when `tz` is `None`.
    pub fn new(expr: &str, tz: Option<&str>) -> Result<Self, CronError> {
        let tz = match tz {
            None => Tz::UTC,
            Some(name) => name
                .parse()
                .map_err(|_| CronError::UnknownTimeZone(name.to_owned()))?,
        };
        let pattern = CronParser::builder()
            .seconds(Seconds::Disallowed)
            .year(Year::Disallowed)
            .build()
            .parse(expr)
            .map_err(|error| CronError::InvalidExpression {
                expr: expr.to_owned(),
                reason: error.to_string(),
            })?;
        let cron = Self { pattern, tz };
        // A pattern such as `0 0 30 2 *` parses but names no real date.
        let reference = DateTime::<Utc>::UNIX_EPOCH;
        if cron.next_after(reference).is_none() {
            return Err(CronError::NeverFires(expr.to_owned()));
        }
        Ok(cron)
    }

    /// The first occurrence strictly after `after`, or `None` when there is none.
    pub fn next_after(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        self.pattern
            .find_next_occurrence(&after.with_timezone(&self.tz), false)
            .ok()
            .map(|next| next.with_timezone(&Utc))
    }

    /// The newest `limit` occurrences from `from` to `to`, both inclusive, oldest first, and
    /// whether older ones in that range were left out.
    pub fn occurrences(
        &self,
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        limit: usize,
    ) -> (Vec<DateTime<Utc>>, bool) {
        let mut newest: Vec<DateTime<Utc>> = self
            .pattern
            .iter_from(to.with_timezone(&self.tz), Direction::Backward)
            .map(|at| at.with_timezone(&Utc))
            .take_while(|at| *at >= from)
            .take(limit.saturating_add(1))
            .collect();
        let truncated = newest.len() > limit;
        newest.truncate(limit);
        newest.reverse();
        (newest, truncated)
    }
}

/// How long the firing of the occurrence at `at` waits: an offset in `[0, jitter)`.
///
/// The offset is derived from `seed` and `at` rather than drawn at random, so every scheduler
/// replica and every test computes the same one, while different Schedules and occurrences
/// spread out.
pub fn jitter(seed: &str, at: DateTime<Utc>, jitter: Duration) -> TimeDelta {
    let window = jitter.as_millis();
    if window == 0 {
        return TimeDelta::zero();
    }
    // FNV-1a: stable across releases and platforms, unlike `std`'s hashers.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in seed
        .bytes()
        .chain([0])
        .chain(at.timestamp_millis().to_le_bytes())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let offset = i64::try_from(hash % window).unwrap_or(i64::MAX);
    TimeDelta::milliseconds(offset)
}

#[cfg(test)]
mod tests {
    use chrono::{TimeDelta, TimeZone};
    use chrono_tz::America::New_York;

    use super::{Cron, CronError, jitter};

    fn utc(rfc3339: &str) -> chrono::DateTime<chrono::Utc> {
        rfc3339.parse().unwrap()
    }

    #[test]
    fn occurrences_keep_their_local_time_across_a_dst_change() {
        let cron = Cron::new("0 9 * * *", Some("America/New_York")).unwrap();

        let before = cron.next_after(utc("2026-03-07T00:00:00Z")).unwrap();
        let after = cron.next_after(before).unwrap();

        assert_eq!(before, utc("2026-03-07T14:00:00Z"), "09:00 EST");
        assert_eq!(after, utc("2026-03-08T13:00:00Z"), "09:00 EDT");
    }

    #[test]
    fn a_time_skipped_by_dst_fires_after_the_gap_and_a_repeated_one_fires_once() {
        let cron = Cron::new("30 2 * * *", Some("America/New_York")).unwrap();
        let skipped = cron.next_after(utc("2026-03-08T05:00:00Z")).unwrap();
        assert_eq!(
            skipped,
            New_York
                .with_ymd_and_hms(2026, 3, 8, 3, 0, 0)
                .unwrap()
                .with_timezone(&chrono::Utc),
        );

        let cron = Cron::new("30 1 * * *", Some("America/New_York")).unwrap();
        let (fall_back, _) =
            cron.occurrences(utc("2026-11-01T00:00:00Z"), utc("2026-11-02T00:00:00Z"), 10);
        assert_eq!(fall_back.len(), 1, "{fall_back:?}");
    }

    #[test]
    fn occurrences_keep_the_newest_within_the_limit() {
        let cron = Cron::new("0 * * * *", None).unwrap();

        let (all, truncated) =
            cron.occurrences(utc("2026-01-01T00:00:00Z"), utc("2026-01-01T03:00:00Z"), 10);
        assert_eq!(all.len(), 4);
        assert!(!truncated);
        assert_eq!(all[0], utc("2026-01-01T00:00:00Z"));

        let (newest, truncated) =
            cron.occurrences(utc("2026-01-01T00:00:00Z"), utc("2026-01-01T03:30:00Z"), 2);
        assert_eq!(
            newest,
            [utc("2026-01-01T02:00:00Z"), utc("2026-01-01T03:00:00Z")]
        );
        assert!(truncated);
    }

    #[test]
    fn bad_expressions_and_time_zones_are_rejected() {
        assert!(matches!(
            Cron::new("0 9 * *", None),
            Err(CronError::InvalidExpression { .. })
        ));
        assert!(
            matches!(
                Cron::new("0 0 9 * * *", None),
                Err(CronError::InvalidExpression { .. })
            ),
            "seconds are not part of the dialect"
        );
        assert_eq!(
            Cron::new("0 0 30 2 *", None).unwrap_err(),
            CronError::NeverFires("0 0 30 2 *".into())
        );
        assert_eq!(
            Cron::new("0 9 * * *", Some("Mars/Olympus")).unwrap_err(),
            CronError::UnknownTimeZone("Mars/Olympus".into())
        );
        assert!(Cron::new("@daily", Some("Europe/Lisbon")).is_ok());
    }

    #[test]
    fn jitter_is_deterministic_and_stays_inside_its_window() {
        let window: crate::dsl::Duration = "10m".parse().unwrap();
        let at = utc("2026-01-01T09:00:00Z");

        assert_eq!(
            jitter("wf/daily", at, window),
            jitter("wf/daily", at, window)
        );
        let offsets: Vec<TimeDelta> = (0..50)
            .map(|hour| jitter("wf/daily", at + TimeDelta::hours(hour), window))
            .collect();
        assert!(
            offsets
                .iter()
                .all(|offset| *offset >= TimeDelta::zero() && *offset < TimeDelta::minutes(10))
        );
        assert!(
            offsets.iter().any(|offset| *offset != offsets[0]),
            "offsets spread out"
        );
        assert_eq!(
            jitter("wf/daily", at, "0s".parse().unwrap()),
            TimeDelta::zero()
        );
    }
}
