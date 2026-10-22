-- Run cancellation (ADR-0036): a flag Workers observe, and a Timeline status
-- for the Attempt or Run it cut short.

ALTER TABLE runs ADD COLUMN cancel_requested_at timestamptz;

ALTER TABLE timeline_entries
    DROP CONSTRAINT timeline_entries_status_check,
    ADD CHECK (status IN ('succeeded', 'failed', 'skipped', 'cancelled'));

-- Tells the Worker running the Run, if any, to cancel its in-flight Attempt.
-- Like `bia_jobs`, it is delivered when the requesting transaction commits.
CREATE FUNCTION runs_notify_cancel_requested() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    PERFORM pg_notify('bia_run_cancels', NEW.id::text);
    RETURN NULL;
END;
$$;

CREATE TRIGGER runs_cancel_requested
    AFTER UPDATE OF cancel_requested_at ON runs
    FOR EACH ROW
    WHEN (OLD.cancel_requested_at IS NULL AND NEW.cancel_requested_at IS NOT NULL)
    EXECUTE FUNCTION runs_notify_cancel_requested();
