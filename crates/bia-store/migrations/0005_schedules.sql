-- Schedules: the scheduler's materialised state for the cron Triggers of
-- Active Versions. Semantics: ADR-0039, ADR-0040.

-- The Active Version the Workflow's Schedules were built from. The scheduler
-- rebuilds them whenever it differs from active_version.
ALTER TABLE workflows ADD COLUMN scheduled_version integer;

CREATE TABLE schedules (
    workflow_id text        NOT NULL,
    trigger_id  text        NOT NULL,
    version     integer     NOT NULL,
    expr        text        NOT NULL,
    tz          text,
    jitter      text,
    overlap     text        NOT NULL CHECK (overlap IN ('skip', 'queue', 'allow')),
    catch_up    text        NOT NULL CHECK (catch_up IN ('none', 'latest', 'all')),
    -- The next occurrence of expr, and when it fires: that plus its jitter.
    next_at     timestamptz NOT NULL,
    due_at      timestamptz NOT NULL CHECK (due_at >= next_at),
    -- Occurrences `overlap: queue` holds back until the previous Run
    -- finishes, oldest first.
    queued      timestamptz[] NOT NULL DEFAULT '{}',
    -- The Run this Schedule started last, which the Overlap Policy watches.
    last_run_id uuid        REFERENCES runs (id) ON DELETE SET NULL,
    PRIMARY KEY (workflow_id, trigger_id),
    FOREIGN KEY (workflow_id, version)
        REFERENCES versions (workflow_id, number) ON DELETE CASCADE
);

CREATE INDEX schedules_by_due ON schedules (due_at);
