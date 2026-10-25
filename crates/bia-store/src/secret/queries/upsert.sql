-- $1: name, $2: ciphertext, $3: nonce, $4: now
INSERT INTO secrets (name, ciphertext, nonce, created_at, updated_at)
VALUES ($1, $2, $3, $4, $4)
ON CONFLICT (name) DO UPDATE
SET ciphertext = EXCLUDED.ciphertext, nonce = EXCLUDED.nonce, updated_at = EXCLUDED.updated_at
RETURNING name, created_at, updated_at
