# Expressions read immutable Outputs and there is no mutable run context

An Expression sees `input`, `steps.<id>.output` (only for Steps that already ran), `trigger`, `run`, and `item`/`index` inside `flow.foreach`. `secrets.NAME` is allowed only in Params and resolved at execution time. There is no `ctx.set`/`ctx.get`. Accumulation happens through aggregated Outputs of `flow.foreach` or `flow.loop_until`. Immutability keeps Checkpoints small and makes a Run explainable from its Timeline alone, which matters most for agents debugging failures.
