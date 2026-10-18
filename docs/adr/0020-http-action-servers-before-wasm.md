# External Actions arrive over HTTP before WASM

The first extensibility mechanism is the Action Server: an HTTP service that registers Actions (name, param schema, output schema, examples) and executes them under a documented contract. It is language-agnostic and immediately lets users write Actions in Python or TypeScript. WASM plugins (`wasmtime`) come afterwards, as their own milestone, for sandboxed in-process extensions.
