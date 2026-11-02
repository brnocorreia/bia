-- Moves a pending request to `withdrawn`, `approved` or `rejected`; updates no row for one
-- that is not pending. $1: id, $2: status, $3: deciding user or NULL, $4: comment, $5: now
UPDATE approval_requests
SET status = $2, decided_by = $3, comment = $4, closed_at = $5
WHERE id = $1 AND status = 'pending'
RETURNING id
