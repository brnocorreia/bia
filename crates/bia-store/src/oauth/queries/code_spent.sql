-- Whether a code that was not redeemed was used before, and the Grant it produced, if any.
-- $1: code hash
SELECT grant_id, code_used_at FROM oauth_authorizations WHERE code_hash = $1
