# Queues are explicit resources and webhooks have fixed paths

A Queue is created explicitly (API, CLI, UI or a YAML resource file) with its own settings. It never springs into existence on first publish. A Definition that references an unknown Queue gets a warning Diagnostic as a Draft and an error at publish time, because implicit creation hides typos behind default settings. For the same reason, webhook Triggers live at `/hooks/{workflow_id}/{trigger_id}` rather than free-form paths, each with its own secret and optional HMAC signature check, so Workflows can never collide on a route.
