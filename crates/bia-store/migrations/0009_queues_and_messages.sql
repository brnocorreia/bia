-- Queues and their Messages (ADR-0015, ADR-0058). A Dead-Letter Queue is a Queue like any
-- other, which another Queue names in `dead_letter`.

CREATE TABLE queues (
    name                  text        PRIMARY KEY,
    max_retries           integer     NOT NULL CHECK (max_retries >= 0),
    visibility_timeout_ms bigint      NOT NULL CHECK (visibility_timeout_ms > 0),
    batch_size            integer     NOT NULL CHECK (batch_size > 0),
    -- A Dead-Letter Queue in use cannot be deleted.
    dead_letter           text        REFERENCES queues (name)
                                      CHECK (dead_letter <> name),
    created_at            timestamptz NOT NULL,
    updated_at            timestamptz NOT NULL
);

CREATE TABLE messages (
    id                 bigint      GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    queue              text        NOT NULL REFERENCES queues (name) ON DELETE CASCADE,
    body               jsonb       NOT NULL,
    published_at       timestamptz NOT NULL,
    -- When the Message can be claimed: its delay or retry backoff, or the end of the
    -- visibility timeout of the delivery holding it. NULL for a dead letter kept in a Queue
    -- without a Dead-Letter Queue, which is never delivered again.
    visible_at         timestamptz,
    -- Deliveries in the current Queue, and the fencing token of the latest one.
    deliveries         integer     NOT NULL DEFAULT 0 CHECK (deliveries >= 0),
    claimed_by         text,
    last_error         text,
    dead_lettered_at   timestamptz,
    dead_lettered_from text,
    CHECK (claimed_by IS NULL OR visible_at IS NOT NULL),
    CHECK ((dead_lettered_at IS NULL) = (dead_lettered_from IS NULL))
);

CREATE INDEX messages_claimable ON messages (queue, visible_at, id) WHERE visible_at IS NOT NULL;
CREATE INDEX messages_by_dead_lettered ON messages (dead_lettered_at)
    WHERE dead_lettered_at IS NOT NULL;
