-- Claims, Leases, retries and wake-ups for Jobs. Semantics: ADR-0031.

ALTER TABLE jobs
    ADD COLUMN max_attempts integer NOT NULL DEFAULT 1 CHECK (max_attempts > 0),
    -- Set when the Job gave up; a failed Job is never claimed again.
    ADD COLUMN failed_at timestamptz,
    ADD CHECK ((locked_by IS NULL) = (locked_until IS NULL)),
    ADD CHECK (failed_at IS NULL OR locked_until IS NULL);

-- The store always binds max_attempts.
ALTER TABLE jobs ALTER COLUMN max_attempts DROP DEFAULT;

DROP INDEX jobs_by_run_at;
CREATE INDEX jobs_claimable ON jobs (run_at, id) WHERE failed_at IS NULL;

-- Wakes idle Workers. The notification is delivered when the enqueuing
-- transaction commits, and Postgres folds duplicates within one transaction.
CREATE FUNCTION jobs_notify_enqueued() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    PERFORM pg_notify('bia_jobs', NEW.kind);
    RETURN NULL;
END;
$$;

CREATE TRIGGER jobs_enqueued
    AFTER INSERT ON jobs
    FOR EACH ROW
    EXECUTE FUNCTION jobs_notify_enqueued();
