-- $1: workflow id, $2: version number, $3: now
UPDATE approval_requests SET status = 'stale', closed_at = $3
WHERE workflow_id = $1 AND number = $2 AND status = 'pending' AND action <> 'activate'
