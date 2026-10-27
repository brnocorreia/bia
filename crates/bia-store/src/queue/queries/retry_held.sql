-- Gives Messages back to their Queue, each claimable again from its own time.
-- $1: message ids, $2: when each is claimable again, $3: error
UPDATE messages
SET run_id = NULL, claimed_by = NULL, visible_at = retry.visible_at, last_error = $3
FROM unnest($1::bigint[], $2::timestamptz[]) AS retry (id, visible_at)
WHERE messages.id = retry.id
