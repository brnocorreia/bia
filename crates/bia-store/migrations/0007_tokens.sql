-- API Tokens and their Scopes (ADR-0008, ADR-0043). Only the SHA-256 of a
-- Token's value is stored; the value is shown once, when the Token is created.

CREATE TABLE tokens (
    id           uuid        PRIMARY KEY DEFAULT gen_random_uuid(),
    name         text        NOT NULL,
    hash         bytea       NOT NULL UNIQUE CHECK (length(hash) = 32),
    scope        text        NOT NULL CHECK (scope IN ('admin', 'agent', 'webhook')),
    -- The webhook Trigger a `webhook` Token may start Runs from. Not a foreign
    -- key: a Token may be minted before its Workflow is created.
    workflow_id  text,
    trigger_id   text,
    created_at   timestamptz NOT NULL,
    last_used_at timestamptz,
    revoked_at   timestamptz,
    CHECK ((scope = 'webhook') = (workflow_id IS NOT NULL)),
    CHECK ((workflow_id IS NULL) = (trigger_id IS NULL))
);

CREATE INDEX tokens_by_created ON tokens (created_at, id);
