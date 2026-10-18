-- Workflows, Versions, Runs, Timeline entries and Jobs. Layout: ADR-0030.
-- Behavioural timestamps are always bound from the Clock (ADR-0022); only
-- `created_at` audit columns default to now().

CREATE TABLE workflows (
    id             text        PRIMARY KEY,
    -- Number of the newest Version; the next one is last_version + 1.
    last_version   integer     NOT NULL DEFAULT 0,
    active_version integer,
    created_at     timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE versions (
    workflow_id  text        NOT NULL REFERENCES workflows (id) ON DELETE CASCADE,
    number       integer     NOT NULL CHECK (number > 0),
    definition   jsonb       NOT NULL,
    state        text        NOT NULL CHECK (state IN ('draft', 'published')),
    published_at timestamptz,
    created_at   timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (workflow_id, number),
    CHECK ((state = 'published') = (published_at IS NOT NULL))
);

ALTER TABLE workflows
    ADD FOREIGN KEY (id, active_version) REFERENCES versions (workflow_id, number);

-- Published Versions never change (ADR-0009).
CREATE FUNCTION versions_reject_published_update() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION 'version % of workflow % is published and cannot be modified',
        OLD.number, OLD.workflow_id
        USING ERRCODE = 'restrict_violation';
END;
$$;

CREATE TRIGGER versions_published_immutable
    BEFORE UPDATE ON versions
    FOR EACH ROW
    WHEN (OLD.state = 'published')
    EXECUTE FUNCTION versions_reject_published_update();

CREATE TABLE runs (
    id           uuid        PRIMARY KEY DEFAULT gen_random_uuid(),
    workflow_id  text        NOT NULL,
    version      integer     NOT NULL,
    status       text        NOT NULL CHECK (status IN (
        'pending', 'running', 'sleeping', 'waiting', 'succeeded', 'failed', 'cancelled'
    )),
    input        jsonb       NOT NULL,
    trigger      jsonb       NOT NULL,
    -- Checkpoint: the Cursor (a path through nested Blocks) and the rest of
    -- the interpreter's state.
    cursor       jsonb       NOT NULL CHECK (jsonb_typeof(cursor) = 'array'),
    state        jsonb       NOT NULL,
    -- Number of Timeline entries; the next one gets seq = timeline_len + 1.
    timeline_len integer     NOT NULL DEFAULT 0,
    created_at   timestamptz NOT NULL,
    updated_at   timestamptz NOT NULL,
    finished_at  timestamptz,
    FOREIGN KEY (workflow_id, version) REFERENCES versions (workflow_id, number),
    CHECK ((status IN ('succeeded', 'failed', 'cancelled')) = (finished_at IS NOT NULL))
);

CREATE INDEX runs_by_created ON runs (created_at DESC, id DESC);
CREATE INDEX runs_by_workflow ON runs (workflow_id, created_at DESC, id DESC);
CREATE INDEX runs_by_status ON runs (status, created_at DESC, id DESC);

-- One row per Attempt.
CREATE TABLE timeline_entries (
    run_id         uuid        NOT NULL REFERENCES runs (id) ON DELETE CASCADE,
    seq            integer     NOT NULL CHECK (seq > 0),
    step_id        text        NOT NULL,
    iteration_path text        NOT NULL,
    attempt        integer     NOT NULL CHECK (attempt > 0),
    action         text        NOT NULL,
    status         text        NOT NULL CHECK (status IN ('succeeded', 'failed', 'skipped')),
    input          jsonb,
    output         jsonb,
    error          jsonb,
    started_at     timestamptz NOT NULL,
    finished_at    timestamptz NOT NULL CHECK (finished_at >= started_at),
    PRIMARY KEY (run_id, seq)
);

CREATE INDEX timeline_entries_by_step ON timeline_entries (run_id, step_id);

CREATE TABLE jobs (
    id           bigint      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    kind         text        NOT NULL CHECK (kind IN ('advance_run')),
    run_id       uuid        REFERENCES runs (id) ON DELETE CASCADE,
    payload      jsonb       NOT NULL DEFAULT '{}',
    -- When the Job becomes claimable; later than enqueue time for delayed Jobs.
    run_at       timestamptz NOT NULL,
    attempts     integer     NOT NULL DEFAULT 0,
    locked_by    text,
    locked_until timestamptz,
    last_error   text,
    created_at   timestamptz NOT NULL DEFAULT now(),
    CHECK (kind <> 'advance_run' OR run_id IS NOT NULL)
);

CREATE INDEX jobs_by_run_at ON jobs (run_at);
CREATE INDEX jobs_by_run ON jobs (run_id);
