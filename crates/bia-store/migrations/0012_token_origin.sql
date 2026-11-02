-- Where a Token came from (ADR-0064). A `session` is a browser sign-in and is the only origin
-- that expires; existing rows were all minted by hand.

ALTER TABLE tokens
    ADD COLUMN origin text NOT NULL DEFAULT 'manual'
        CHECK (origin IN ('session', 'oauth', 'manual'));

ALTER TABLE tokens ALTER COLUMN origin DROP DEFAULT;
