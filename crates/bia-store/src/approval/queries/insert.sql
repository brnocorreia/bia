-- $1: workflow id, $2: version number, $3: action, $4: note, $5: content hash,
-- $6: requesting user, $7: requesting Token, $8: now
INSERT INTO approval_requests (workflow_id, number, action, note, content_hash, status,
                               requested_by, requester_token, created_at)
VALUES ($1, $2, $3, $4, $5, 'pending', $6, $7, $8)
RETURNING id
