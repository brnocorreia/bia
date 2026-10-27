-- Queue Triggers (ADR-0061): the scheduler's materialised state for the queue Triggers of
-- Active Versions, and the Run each delivered Message is held by.

-- The Run a queue Trigger delivered the Message to. The Message stays held, whatever its
-- visible_at says, until that Run's last transition acks or nacks it.
ALTER TABLE messages
    ADD COLUMN run_id uuid REFERENCES runs (id) ON DELETE SET NULL,
    ADD CHECK (run_id IS NULL OR claimed_by IS NOT NULL);

DROP INDEX messages_claimable;
CREATE INDEX messages_claimable ON messages (queue, visible_at, id)
    WHERE visible_at IS NOT NULL AND run_id IS NULL;
CREATE INDEX messages_by_run ON messages (run_id) WHERE run_id IS NOT NULL;

CREATE TABLE queue_triggers (
    workflow_id text    NOT NULL,
    trigger_id  text    NOT NULL,
    version     integer NOT NULL,
    -- No foreign key: a Queue can be deleted while an Active Version still names it.
    queue       text    NOT NULL,
    -- NULL takes the Queue's batch_size at each dispatch.
    batch_size  integer CHECK (batch_size > 0),
    PRIMARY KEY (workflow_id, trigger_id),
    FOREIGN KEY (workflow_id, version)
        REFERENCES versions (workflow_id, number) ON DELETE CASCADE
);

CREATE INDEX queue_triggers_by_queue ON queue_triggers (queue);

-- Rebuilds every Workflow's Trigger state on the next poll, so Active Versions activated
-- before this migration get their queue Triggers. Cron Schedules whose timing is unchanged
-- keep their next occurrence (ADR-0039).
UPDATE workflows SET scheduled_version = NULL;
