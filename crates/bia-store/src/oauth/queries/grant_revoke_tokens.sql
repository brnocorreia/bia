-- $1: grant id, $2: now
UPDATE tokens SET revoked_at = coalesce(revoked_at, $2) WHERE grant_id = $1
