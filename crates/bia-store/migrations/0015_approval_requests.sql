-- Approval Requests: an agent asks a human to publish and/or activate a Version (ADR-0066).
-- Status changes are behavioural, so `closed_at` is bound from the Clock (ADR-0022).

CREATE TABLE approval_requests (
    id              uuid        PRIMARY KEY DEFAULT gen_random_uuid(),
    workflow_id     text        NOT NULL,
    number          integer     NOT NULL,
    action          text        NOT NULL CHECK (action IN ('publish', 'publish_and_activate', 'activate')),
    note            text,
    -- SHA-256 of the Draft's Definition the requester asked about; NULL for `activate`.
    content_hash    text,
    status          text        NOT NULL CHECK (status IN (
        'pending', 'approved', 'rejected', 'stale', 'withdrawn'
    )),
    requested_by    uuid        NOT NULL REFERENCES users (id),
    requester_token uuid        NOT NULL REFERENCES tokens (id),
    decided_by      uuid        REFERENCES users (id),
    comment         text,
    created_at      timestamptz NOT NULL,
    -- When the status left `pending`.
    closed_at       timestamptz,
    FOREIGN KEY (workflow_id, number) REFERENCES versions (workflow_id, number) ON DELETE CASCADE,
    CHECK ((status = 'pending') = (closed_at IS NULL)),
    CHECK ((status IN ('approved', 'rejected')) = (decided_by IS NOT NULL)),
    CHECK ((action = 'activate') = (content_hash IS NULL))
);

-- One pending request per Workflow: a newer one withdraws the older.
CREATE UNIQUE INDEX approval_requests_one_pending
    ON approval_requests (workflow_id) WHERE status = 'pending';

CREATE INDEX approval_requests_by_created ON approval_requests (created_at DESC, id);
