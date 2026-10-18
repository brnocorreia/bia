-- $1: workflow id
SELECT EXISTS (SELECT 1 FROM workflows WHERE id = $1) AS "exists!"
