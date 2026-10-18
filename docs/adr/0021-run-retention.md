# Runs are retained for 90 days by default

Finished Runs and their Timelines are deleted after `BIA_RUN_RETENTION_DAYS` (default 90) by an internal job that bia's own scheduler fires. Failed Runs and Dead-Letter Queue contents follow a separate, independently configurable retention (also default 90).
