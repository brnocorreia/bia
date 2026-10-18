# The DSL is a tree of nested Blocks, not a graph

`steps` is an ordered list. Control-flow Steps (Blocks) contain nested Step lists (`then`, `else`, `branches`, `body`, `catch`) instead of referencing other Steps by id. Without gotos or `depends_on`, cycles and orphan Steps cannot be expressed, so validation needs no graph analysis. LLMs also generate trees more reliably than cross-referenced graphs, and a Run's Cursor is a simple path (e.g. `steps[2].then[0]`). Parallelism is explicit through `flow.parallel` and `flow.foreach`.

## Considered Options

- Step Functions-style states with `next`: very flexible, but turns every Definition into a state graph to validate and render.
- DAG with `depends_on`: implicit parallelism, but harder Checkpoint semantics and harder for agents to author correctly.
