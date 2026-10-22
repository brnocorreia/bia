//! The `Clock` every component reads time from (ADR-0022).
//!
//! This crate only defines the trait and a manual Clock for tests: reading the system time is
//! IO, so the system Clock lives in `bia-store`.

use std::fmt;
use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::action::BoxFuture;

/// Where the engine, scheduler and store read the current time from, and wait on it.
pub trait Clock: fmt::Debug + Send + Sync {
    fn now(&self) -> DateTime<Utc>;

    /// Completes once the Clock reads `deadline` or later, so in-process waits such as an
    /// Attempt's timeout follow the same time as everything else (ADR-0034).
    fn sleep_until(&self, deadline: DateTime<Utc>) -> BoxFuture<'static, ()>;
}

impl<C: Clock + ?Sized> Clock for &C {
    fn now(&self) -> DateTime<Utc> {
        (**self).now()
    }

    fn sleep_until(&self, deadline: DateTime<Utc>) -> BoxFuture<'static, ()> {
        (**self).sleep_until(deadline)
    }
}

impl<C: Clock + ?Sized> Clock for Arc<C> {
    fn now(&self) -> DateTime<Utc> {
        (**self).now()
    }

    fn sleep_until(&self, deadline: DateTime<Utc>) -> BoxFuture<'static, ()> {
        (**self).sleep_until(deadline)
    }
}

#[cfg(feature = "testing")]
pub use manual::ManualClock;

#[cfg(feature = "testing")]
mod manual {
    use std::sync::Arc;

    use chrono::{DateTime, TimeDelta, Utc};
    use tokio::sync::watch;

    use super::Clock;
    use crate::action::BoxFuture;

    /// A Clock that only moves when a test moves it, behind the `testing` feature. Clones
    /// share the same time, so a test can hand one to the code under test and advance another.
    /// Sleeps complete when the time is moved to or past their deadline.
    #[derive(Debug, Clone)]
    pub struct ManualClock {
        now: Arc<watch::Sender<DateTime<Utc>>>,
    }

    impl ManualClock {
        pub fn new(start: DateTime<Utc>) -> Self {
            Self {
                now: Arc::new(watch::Sender::new(start)),
            }
        }

        /// Moves time forward by `by`, or backward when `by` is negative.
        pub fn advance(&self, by: TimeDelta) {
            self.now.send_modify(|now| *now += by);
        }

        /// Jumps to `to`.
        pub fn set(&self, to: DateTime<Utc>) {
            self.now.send_replace(to);
        }
    }

    impl Clock for ManualClock {
        fn now(&self) -> DateTime<Utc> {
            *self.now.borrow()
        }

        fn sleep_until(&self, deadline: DateTime<Utc>) -> BoxFuture<'static, ()> {
            let mut now = self.now.subscribe();
            Box::pin(async move {
                if now.wait_for(|now| *now >= deadline).await.is_err() {
                    // Every handle on the Clock is gone, so time never reaches the deadline.
                    std::future::pending::<()>().await;
                }
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use std::task::{Context, Waker};

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

    #[test]
    fn a_manual_sleep_completes_once_time_reaches_its_deadline() {
        let start = chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap();
        let clock = ManualClock::new(start);
        let mut sleep = clock.sleep_until(start + TimeDelta::seconds(30));
        let mut context = Context::from_waker(Waker::noop());

        assert!(sleep.as_mut().poll(&mut context).is_pending());
        clock.advance(TimeDelta::seconds(29));
        assert!(sleep.as_mut().poll(&mut context).is_pending());
        clock.advance(TimeDelta::seconds(1));
        assert!(sleep.as_mut().poll(&mut context).is_ready());

        let mut past = clock.sleep_until(start);
        assert!(past.as_mut().poll(&mut context).is_ready());
    }
}
