-- The device authorization grant (ADR-0068): a CLI on a machine without a browser asks for a
-- device code, a person approves a short user code on another device, and the CLI polls. Both
-- codes are stored as SHA-256 hashes only, like Tokens.

CREATE TABLE oauth_device_authorizations (
    id               uuid        PRIMARY KEY DEFAULT gen_random_uuid(),
    client_id        text        NOT NULL,
    client_name      text        NOT NULL,
    client_host      text,
    provenance       text        NOT NULL CHECK (provenance IN ('cimd', 'dynamic', 'builtin')),
    requested_scope  text        NOT NULL CHECK (requested_scope IN ('admin', 'agent')),
    resource         text        NOT NULL,
    device_code_hash bytea       NOT NULL UNIQUE CHECK (length(device_code_hash) = 32),
    -- The hash of the normalised user code. One user code names one authorization.
    user_code_hash   bytea       NOT NULL UNIQUE CHECK (length(user_code_hash) = 32),
    created_at       timestamptz NOT NULL,
    expires_at       timestamptz NOT NULL,
    -- Seconds the CLI must leave between polls; raised when it polls too fast.
    interval_secs    integer     NOT NULL CHECK (interval_secs > 0),
    last_polled_at   timestamptz,
    -- Set once the User approved or denied; a request is decided at most once.
    decided_at       timestamptz,
    outcome          text        CHECK (outcome IN ('approved', 'denied')),
    user_id          uuid        REFERENCES users (id),
    granted_scope    text        CHECK (granted_scope IN ('admin', 'agent')),
    -- Set when the device code is exchanged, which it is only once.
    consumed_at      timestamptz,
    -- The Grant it was exchanged for, so a replayed device code can revoke it.
    grant_id         uuid        REFERENCES grants (id)
);
