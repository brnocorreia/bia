-- Records that the Chore runs at $2 unless it already ran after $3. Concurrent claimers
-- queue on the row, and the later ones then see the new ran_at and get no row back.
-- $1: chore name, $2: now, $3: claim only if it last ran at or before this
INSERT INTO chores (name, ran_at)
VALUES ($1, $2)
ON CONFLICT (name) DO UPDATE
SET ran_at = EXCLUDED.ran_at
WHERE chores.ran_at <= $3
RETURNING ran_at
