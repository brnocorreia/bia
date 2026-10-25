-- Serialises creating the first admin Token until the transaction ends.
-- $1: the advisory lock key
SELECT pg_advisory_xact_lock($1)
