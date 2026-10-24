-- When each Chore last ran, so it runs once per interval across schedulers
-- (ADR-0041), and the index Run retention deletes by (ADR-0021).

CREATE TABLE chores (
    name   text        PRIMARY KEY,
    ran_at timestamptz NOT NULL
);

CREATE INDEX runs_by_finished ON runs (status, finished_at) WHERE finished_at IS NOT NULL;
