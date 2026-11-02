-- $1: id
SELECT a.id, a.workflow_id, a.number, a.action, a.note, a.content_hash, a.status,
       a.requested_by, a.requester_token, t.name AS requester_name, a.decided_by, a.comment,
       a.created_at, a.closed_at
FROM approval_requests a
JOIN tokens t ON t.id = a.requester_token
WHERE a.id = $1
