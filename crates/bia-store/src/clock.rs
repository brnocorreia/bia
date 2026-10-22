//! The system Clock. The `Clock` trait lives in `bia-core`, which reads no time itself.

use bia_core::action::BoxFuture;
use bia_core::clock::Clock;
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    #[allow(clippy::disallowed_methods)]
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }

    fn sleep_until(&self, deadline: DateTime<Utc>) -> BoxFuture<'static, ()> {
        let wait = (deadline - self.now()).to_std().unwrap_or_default();
        Box::pin(tokio::time::sleep(wait))
    }
}
