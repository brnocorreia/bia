-- OAuth for MCP (ADR-0067): clients that registered themselves, the Grants a User approved,
-- the authorization requests that lead to one, and the refresh tokens that renew it. Codes,
-- access tokens and refresh tokens are stored as SHA-256 hashes only, like Tokens.

CREATE TABLE oauth_clients (
    client_id     text        PRIMARY KEY,
    client_name   text        NOT NULL,
    redirect_uris text[]      NOT NULL CHECK (cardinality(redirect_uris) > 0),
    created_at    timestamptz NOT NULL
);

CREATE TABLE grants (
    id           uuid        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id      uuid        NOT NULL REFERENCES users (id),
    client_id    text        NOT NULL,
    client_name  text        NOT NULL,
    -- The host a metadata document was fetched from, for the consent screen and the Access one.
    client_host  text,
    provenance   text        NOT NULL CHECK (provenance IN ('cimd', 'dynamic', 'builtin')),
    scope        text        NOT NULL CHECK (scope IN ('admin', 'agent')),
    -- The audience of every Token the Grant issues.
    resource     text        NOT NULL,
    created_at   timestamptz NOT NULL,
    last_used_at timestamptz,
    revoked_at   timestamptz
);

CREATE INDEX grants_by_created ON grants (created_at, id);

ALTER TABLE tokens
    ADD CONSTRAINT tokens_grant_fk FOREIGN KEY (grant_id) REFERENCES grants (id),
    -- Access tokens expire; Sessions are judged by their own rules and manual Tokens never end.
    ADD COLUMN expires_at timestamptz,
    -- Copied from the Grant: the resource the Token may be used at. NULL for no restriction.
    ADD COLUMN audience text;

CREATE INDEX tokens_by_grant ON tokens (grant_id) WHERE grant_id IS NOT NULL;

CREATE TABLE oauth_authorizations (
    id              uuid        PRIMARY KEY DEFAULT gen_random_uuid(),
    client_id       text        NOT NULL,
    client_name     text        NOT NULL,
    client_host     text,
    provenance      text        NOT NULL CHECK (provenance IN ('cimd', 'dynamic', 'builtin')),
    redirect_uri    text        NOT NULL,
    requested_scope text        NOT NULL CHECK (requested_scope IN ('admin', 'agent')),
    resource        text        NOT NULL,
    state           text,
    code_challenge  text        NOT NULL,
    created_at      timestamptz NOT NULL,
    expires_at      timestamptz NOT NULL,
    -- Set once the User approved or denied; a request is decided at most once.
    decided_at      timestamptz,
    user_id         uuid        REFERENCES users (id),
    granted_scope   text        CHECK (granted_scope IN ('admin', 'agent')),
    code_hash       bytea       UNIQUE CHECK (length(code_hash) = 32),
    code_expires_at timestamptz,
    code_used_at    timestamptz,
    -- The Grant the code was exchanged for, so a replayed code can revoke it.
    grant_id        uuid        REFERENCES grants (id)
);

CREATE TABLE grant_refresh_tokens (
    id         uuid        PRIMARY KEY DEFAULT gen_random_uuid(),
    grant_id   uuid        NOT NULL REFERENCES grants (id),
    hash       bytea       NOT NULL UNIQUE CHECK (length(hash) = 32),
    created_at timestamptz NOT NULL,
    expires_at timestamptz NOT NULL,
    -- Set when it is exchanged. Presenting a used one again means it leaked.
    used_at    timestamptz
);

CREATE INDEX grant_refresh_tokens_by_grant ON grant_refresh_tokens (grant_id);
