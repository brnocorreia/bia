-- Secrets of the internal Secret Provider (ADR-0017, ADR-0049). The value is
-- encrypted with XChaCha20-Poly1305 under BIA_MASTER_KEY; `name` is the
-- associated data, so a ciphertext cannot be moved to another name.

CREATE TABLE secrets (
    name       text        PRIMARY KEY,
    ciphertext bytea       NOT NULL,
    nonce      bytea       NOT NULL CHECK (length(nonce) = 24),
    created_at timestamptz NOT NULL,
    updated_at timestamptz NOT NULL
);
