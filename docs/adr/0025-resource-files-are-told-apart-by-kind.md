# Resource files are told apart by a top-level `kind`

A file applied with the CLI holds either a Workflow Definition or a resource such as a Queue. Resources carry a top-level `kind` (`kind: queue`), and Workflow Definitions never have one. A document without `kind` is a Definition and must carry `schema_version`. This keeps every existing Definition valid as written, and the `kind` field is free to grow with new resource types. Requiring `kind: workflow` on Definitions would have been more uniform, but it would add a mandatory field to every Definition for the benefit of the CLI alone.
