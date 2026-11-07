-- What the Access screen needs to tell credentials apart: the browser behind a Session, and the
-- OAuth Grant behind an `oauth` Token. There is no foreign key on `grant_id` until Grants exist.

ALTER TABLE tokens
    ADD COLUMN user_agent text,
    ADD COLUMN grant_id uuid;
