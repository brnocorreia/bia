-- Withdraws the Workflow's pending request, if any. $1: workflow id, $2: now
UPDATE approval_requests SET status = 'withdrawn', closed_at = $2
WHERE workflow_id = $1 AND status = 'pending'
