-- $1: name
DELETE FROM queues
WHERE name = $1
