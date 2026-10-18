//! The `Clock` every component reads time from (ADR-0022).
//!
//! This crate only defines the trait and a manual Clock for tests: reading the system time is
//! IO, so the system Clock lives in `bia-store`.

use std::fmt;
use std::sync::Arc;

use chrono::{DateTime, Utc};

/// Where the engine, scheduler and store read the current time from.
pub trait Clock: fmt::Debug + Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

impl<C: Clock + ?Sized> Clock for &C {
    fn now(&self) -> DateTime<Utc> {
        (**self).now()
    }
}

impl<C: Clock + ?Sized> Clock for Arc<C> {
    fn now(&self) -> DateTime<Utc> {
        (**self).now()
    }
}

#[cfg(feature = "testing")]
pub use manual::ManualClock;

#[cfg(feature = "testing")]
mod manual {
    use std::sync::{Arc, Mutex, PoisonError};

    use chrono::{DateTime, TimeDelta, Utc};

    use super::Clock;

    /// A Clock that only moves when a test moves it, behind the `testing` feature. Clones
    /// share the same time, so a test can hand one to the code under test and advance another.
    #[derive(Debug, Clone)]
    pub struct ManualClock {
        now: Arc<Mutex<DateTime<Utc>>>,
    }

    impl ManualClock {
        pub fn new(start: DateTime<Utc>) -> Self {
            Self {
                now: Arc::new(Mutex::new(start)),
            }
        }

        /// Moves time forward by `by`, or backward when `by` is negative.
        pub fn advance(&self, by: TimeDelta) {
            *self.now.lock().unwrap_or_else(PoisonError::into_inner) += by;
        }

        /// Jumps to `to`.
        pub fn set(&self, to: DateTime<Utc>) {
            *self.now.lock().unwrap_or_else(PoisonError::into_inner) = to;
        }
    }

    impl Clock for ManualClock {
        fn now(&self) -> DateTime<Utc> {
            *self.now.lock().unwrap_or_else(PoisonError::into_inner)
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeDelta;

    use super::{Clock, ManualClock};

    #[test]
    fn a_manual_clock_moves_only_when_told_and_clones_share_its_time() {
        let start = chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        let clock = ManualClock::new(start);
        let handed_out = clock.clone();
        assert_eq!(handed_out.now(), start);

        clock.advance(TimeDelta::days(3));
        assert_eq!(handed_out.now(), start + TimeDelta::days(3));

        clock.set(start);
        assert_eq!(handed_out.now(), start);
    }
}
