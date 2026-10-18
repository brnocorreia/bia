//! The system Clock. The `Clock` trait lives in `bia-core`, which reads no time itself.

use bia_core::clock::Clock;
use chrono::{DateTime, Utc};

/// The Clock production uses: the host's wall-clock time.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    #[allow(clippy::disallowed_methods)]
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}
