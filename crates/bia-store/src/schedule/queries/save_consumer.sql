-- $1: workflow id, $2: trigger id, $3: version, $4: queue, $5: batch size, NULL for the Queue's
INSERT INTO queue_triggers (workflow_id, trigger_id, version, queue, batch_size)
VALUES ($1, $2, $3, $4, $5)
