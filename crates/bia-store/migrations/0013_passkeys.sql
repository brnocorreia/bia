-- Passkeys a User signs in with instead of the password (ADR-0065).

CREATE TABLE passkeys (
    id           uuid        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id      uuid        NOT NULL REFERENCES users (id),
    name         text        NOT NULL,
    -- The serialised `webauthn-rs` credential: public key, signature counter, flags. Opaque here.
    credential   jsonb       NOT NULL,
    created_at   timestamptz NOT NULL,
    last_used_at timestamptz
);

CREATE INDEX passkeys_by_user ON passkeys (user_id);

-- The server-side state of a WebAuthn ceremony between its `begin` and `finish` calls. A row is
-- used once and judged against `expires_at` with the injected Clock; a row that was never
-- finished is deleted by the next `begin`.
CREATE TABLE passkey_ceremonies (
    id         uuid        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id    uuid        NOT NULL REFERENCES users (id),
    kind       text        NOT NULL CHECK (kind IN ('registration', 'authentication')),
    state      jsonb       NOT NULL,
    expires_at timestamptz NOT NULL
);
