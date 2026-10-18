# Time comes from an injected Clock

The engine, scheduler and store take a `Clock` (a trait in `bia-core`) instead of reading system time. Queries compare against the time the Clock supplies rather than Postgres `now()`. Durable sleeps of days, cron firing and catch-up, Lease expiry and 90-day retention are only testable if tests can move time forward deterministically, and a manual test Clock makes them fast and exact. Production uses the system clock. Timestamps that are pure audit metadata (row `created_at` defaults) may still come from Postgres.
