//! Sign-in throttling (ADR-0064): after a few failures, each further attempt waits longer.
//!
//! Counted in memory, per User and client address, so a restart or another API process starts
//! from zero. A wait slows an attacker down without locking the admin out of their own
//! install.

use std::collections::HashMap;
use std::convert::Infallible;
use std::fmt;
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::Mutex;
use std::time::Duration;

use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::request::Parts;
use bia_store::user::UserId;
use chrono::{DateTime, TimeDelta, Utc};

use crate::ApiState;

/// Attempts that fail without a wait.
const FREE_FAILURES: u32 = 5;
const MAX_WAIT: Duration = Duration::from_secs(60);
/// A client that stays away this long starts again from zero.
const FORGET_AFTER: TimeDelta = TimeDelta::hours(1);

/// How a sign-in waits out its penalty. The server sleeps; a test records the wait instead,
/// so it need not sleep.
pub trait Wait: fmt::Debug + Send + Sync {
    fn wait(&self, duration: Duration) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

#[derive(Debug)]
pub(crate) struct TokioWait;

impl Wait for TokioWait {
    fn wait(&self, duration: Duration) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(tokio::time::sleep(duration))
    }
}

/// Who an attempt counts against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Subject {
    user: UserId,
    address: IpAddr,
}

impl Subject {
    pub(crate) fn new(user: UserId, address: IpAddr) -> Self {
        Self { user, address }
    }
}

#[derive(Debug, Default)]
pub(crate) struct SignInThrottle {
    attempts: Mutex<HashMap<Subject, Attempts>>,
}

#[derive(Debug)]
struct Attempts {
    /// Attempts since the last success, counting those still being checked, so a burst of
    /// parallel guesses waits like a sequence of them.
    count: u32,
    last: DateTime<Utc>,
}

impl SignInThrottle {
    /// Counts an attempt as a failure until [`succeeded`](Self::succeeded) says otherwise, and
    /// returns how long it must wait before it is checked.
    pub(crate) fn begin(&self, subject: Subject, now: DateTime<Utc>) -> Duration {
        let mut attempts = self
            .attempts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        attempts.retain(|_, attempts| now - attempts.last < FORGET_AFTER);
        let entry = attempts.entry(subject).or_insert(Attempts {
            count: 0,
            last: now,
        });
        let wait = wait_after(entry.count);
        entry.count = entry.count.saturating_add(1);
        entry.last = now;
        wait
    }

    pub(crate) fn succeeded(&self, subject: Subject) {
        self.attempts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&subject);
    }
}

/// 1 s after the fifth failure, doubling up to a minute.
fn wait_after(failures: u32) -> Duration {
    let Some(extra) = failures.checked_sub(FREE_FAILURES) else {
        return Duration::ZERO;
    };
    Duration::from_secs(1u64.checked_shl(extra).unwrap_or(u64::MAX)).min(MAX_WAIT)
}

impl ApiState {
    /// Counts a sign-in attempt, waits out its penalty, and returns the subject to pass to
    /// [`SignInThrottle::succeeded`] when the password was right.
    pub(crate) async fn throttle(&self, user: UserId, address: IpAddr) -> Subject {
        let subject = Subject::new(user, address);
        let wait = self.sign_ins.begin(subject, self.clock.now());
        if !wait.is_zero() {
            tracing::warn!(wait_secs = wait.as_secs(), "sign-in attempts are throttled");
            self.config.wait().wait(wait).await;
        }
        subject
    }
}

/// The address the request came from, or the unspecified one when the server was not told.
/// Behind a proxy this is the proxy's, so all callers share one count; forwarded headers are
/// not trusted, since anyone could forge them.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ClientAddress(pub(crate) IpAddr);

impl<S: Send + Sync> FromRequestParts<S> for ClientAddress {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Infallible> {
        Ok(Self(
            parts
                .extensions
                .get::<ConnectInfo<SocketAddr>>()
                .map_or(IpAddr::from([0, 0, 0, 0]), |info| info.0.ip()),
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::net::IpAddr;
    use std::time::Duration;

    use bia_store::user::UserId;
    use chrono::{DateTime, TimeDelta, Utc};
    use uuid::Uuid;

    use super::{SignInThrottle, Subject, wait_after};

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000, 0).unwrap_or_default()
    }

    fn subject(last_byte: u8) -> Subject {
        Subject::new(
            UserId::from_uuid(Uuid::nil()),
            IpAddr::from([10, 0, 0, last_byte]),
        )
    }

    #[test]
    fn the_wait_starts_after_the_fifth_failure_and_doubles_up_to_a_minute() {
        let waits: Vec<u64> = (0..13).map(|n| wait_after(n).as_secs()).collect();

        assert_eq!(waits, [0, 0, 0, 0, 0, 1, 2, 4, 8, 16, 32, 60, 60]);
        assert_eq!(wait_after(u32::MAX), Duration::from_secs(60));
    }

    #[test]
    fn the_sixth_attempt_waits_and_a_success_starts_again() {
        let throttle = SignInThrottle::default();
        let waits: Vec<u64> = (0..7)
            .map(|_| throttle.begin(subject(1), now()).as_secs())
            .collect();
        assert_eq!(waits, [0, 0, 0, 0, 0, 1, 2]);

        throttle.succeeded(subject(1));

        assert_eq!(throttle.begin(subject(1), now()), Duration::ZERO);
    }

    #[test]
    fn each_address_and_a_long_absence_count_separately() {
        let throttle = SignInThrottle::default();
        for _ in 0..6 {
            throttle.begin(subject(1), now());
        }

        assert_eq!(throttle.begin(subject(2), now()), Duration::ZERO);
        let later = now() + TimeDelta::hours(2);
        assert_eq!(throttle.begin(subject(1), later), Duration::ZERO);
    }
}
