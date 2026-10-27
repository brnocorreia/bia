-- The people who sign in, each owning their Tokens (ADR-0062). First-run setup creates the one
-- User under an advisory lock, so the table holds at most one row.

CREATE TABLE users (
    id            uuid        PRIMARY KEY DEFAULT gen_random_uuid(),
    -- argon2id, as a PHC string. NULL after `bia serve --reset-admin` until setup sets it again.
    password_hash text,
    created_at    timestamptz NOT NULL
);

-- Tokens minted with the Bootstrap Key belong to no User; nothing depends on them yet.
DELETE FROM tokens;

ALTER TABLE tokens ADD COLUMN user_id uuid NOT NULL REFERENCES users (id);

CREATE INDEX tokens_by_user ON tokens (user_id);
