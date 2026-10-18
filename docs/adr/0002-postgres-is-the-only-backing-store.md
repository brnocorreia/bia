# Postgres is the only backing store

The Workflow registry, Run state, Timelines, Queues, Jobs and Schedules all live in Postgres. Claims use `SELECT … FOR UPDATE SKIP LOCKED`, Leases use a `locked_until` column, the scheduler uses advisory locks, and `LISTEN/NOTIFY` wakes idle Workers. Having one store means a Checkpoint, a Job completion and the next Job's enqueue can commit in one transaction.

## Considered Options

- SQLite as an alternative target: dropped. Abstracting over both would cost the transactional and locking features this design relies on.
- Redis or NATS for queues: rejected. A second store breaks the single-transaction guarantee.
