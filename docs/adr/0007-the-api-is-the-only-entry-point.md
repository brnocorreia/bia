# The HTTP API is the only entry point

The UI, the CLI's operational commands, and the MCP server all go through the same HTTP API, via the typed `bia-client` crate for Rust callers. MCP is served over streamable HTTP at `/mcp` by `bia serve`, and `bia mcp` is a stdio proxy for local MCP clients. Only `bia serve` and `bia migrate` touch Postgres directly. Authorisation rules such as "agents cannot publish" (ADR-0008) therefore hold for every caller, because no caller can bypass them.
