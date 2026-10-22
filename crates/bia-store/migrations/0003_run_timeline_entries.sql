-- Timeline entries about the Run as a whole, such as input rejected before any
-- Step ran, have no Step, Attempt or Action. Shape: ADR-0033.

ALTER TABLE timeline_entries
    ALTER COLUMN step_id DROP NOT NULL,
    ALTER COLUMN attempt DROP NOT NULL,
    ALTER COLUMN action DROP NOT NULL,
    ADD CHECK ((step_id IS NULL) = (attempt IS NULL)),
    ADD CHECK ((step_id IS NULL) = (action IS NULL)),
    ADD CHECK (step_id IS NOT NULL OR iteration_path = '');
