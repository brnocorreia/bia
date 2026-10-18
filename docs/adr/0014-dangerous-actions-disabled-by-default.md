# Dangerous Actions are disabled by default and never agent-publishable

`exec.*`, `container.*` and `ssh.*` ship in the catalogue but are off unless enabled in server config, under a policy: `exec.process` uses a binary allowlist and no shell; `exec.shell` sits behind its own explicit flag; `container.run` applies CPU, memory and network limits. Validation refuses to publish a Version that uses a disabled Action, and Versions using these Actions can only be published with an `admin` Token.
