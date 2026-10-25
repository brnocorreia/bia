# Quickstart: the anchor scenario

This walks the scenario bia must support: an agent, over MCP, authors an uptime-check Workflow, fixes a Diagnostic and dry-runs it; a human publishes it; cron runs it; the agent inspects the failed Runs and retries one. The automated version of the same scenario is `crates/bia-cli/tests/anchor_scenario.rs`.

You need Rust (the pinned toolchain installs itself), Docker, `openssl`, `python3`, and a clone of this repository. Run every command from the repository root unless a step says otherwise.

## 1. Install and start bia

```sh
cargo install --path crates/bia-cli --locked      # installs the `bia` binary
docker compose up --detach --wait postgres        # Postgres on localhost:5432

export DATABASE_URL=postgres://bia:bia@localhost:5432/bia
export BIA_BOOTSTRAP_KEY="$(openssl rand -hex 32)"   # lets you mint the first admin Token
export BIA_MASTER_KEY="$(openssl rand -hex 32)"      # encrypts Secrets; back it up

bia migrate
bia serve --role all
```

`bia serve --role all` keeps running, so use a second terminal for the rest. Both keys are only read by the server. See [`configuration.md`](configuration.md) for every setting.

## 2. Create the Tokens

In the second terminal, `BIA_URL` defaults to `http://127.0.0.1:8080`, so only `BIA_BOOTSTRAP_KEY` needs to be set again, with the value you exported for the server:

```sh
export BIA_BOOTSTRAP_KEY=<the same value as in the first terminal>
export BIA_TOKEN="$(bia token create --bootstrap --name admin)"    # the admin Token
bia token create --name claude --scope agent                       # prints the agent Token once
```

Keep the admin Token for yourself. Copy the agent Token's value (it starts with `bia_`) for the MCP client. An `agent` Token can author Drafts, validate, dry-run, trigger and read, but cannot publish or activate (ADR-0008).

## 3. Set the Secret and start a target

The uptime check calls a target with a bearer token. Store that token as a Secret, which only `admin` can set and nobody can read back:

```sh
printf '%s' 's3cret-uptime-token' | bia secret set API_TOKEN
```

The target is a stand-in that answers `503` until a file named `up` exists in its working directory. Save this as `target.py` in an empty directory and run `python3 target.py` in a third terminal:

```python
import json, os
from http.server import BaseHTTPRequestHandler, HTTPServer


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.headers.get("Authorization") != "Bearer s3cret-uptime-token":
            self.send_response(401)
            self.end_headers()
            return
        up = os.path.exists("up")
        self.send_response(200 if up else 503)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(json.dumps({"status": "up" if up else "down"}).encode())


HTTPServer(("127.0.0.1", 9000), Handler).serve_forever()
```

## 4. Connect an MCP client

`bia mcp` is a stdio MCP server that forwards to `bia serve` with the Token in `BIA_TOKEN`. Give it the agent Token, never the admin one.

Claude Code:

```sh
claude mcp add bia --env BIA_URL=http://127.0.0.1:8080 --env BIA_TOKEN=<agent token> -- bia mcp
```

Claude Desktop: add this under `mcpServers` in `claude_desktop_config.json`, with the absolute path of `bia` if it is not on the app's `PATH`:

```json
{ "bia": { "command": "bia", "args": ["mcp"], "env": { "BIA_URL": "http://127.0.0.1:8080", "BIA_TOKEN": "<agent token>" } } }
```

Any client that speaks streamable HTTP can instead connect to `http://127.0.0.1:8080/mcp` with the header `Authorization: Bearer <agent token>`. The agent sees these tools: `get_schema`, `list_actions`, `list_workflows`, `get_workflow`, `list_versions`, `get_version`, `validate_workflow`, `create_draft`, `replace_draft`, `dry_run`, `diff_versions`, `trigger_run`, `get_run`, `list_runs`, `list_failures`, `retry_run`, `resume_run`, `cancel_run` and `list_secrets`. There is deliberately no tool to publish or activate.

## 5. Walk the scenario

Each step lists what to ask the agent and, below it, the tool calls it makes. If you have no MCP client, [Driving the tools by hand](#driving-the-tools-by-hand) runs the same calls from a shell.

**1. Author a Draft.** Ask: _"Using bia, create a Workflow `uptime-check` that runs every minute, calls `http://127.0.0.1:9000/health` with the header `Authorization: Bearer {{ secrets.API_TOKEN }}`, and times out after 10 seconds."_ The agent calls `get_schema` and `list_actions`, then `create_draft`. Expect a Draft, Version 1. A typo such as `http.requst` is accepted in a Draft.

**2. Fix the Diagnostic.** The agent calls `validate_workflow` with `target: "publish"`. A typo comes back as a Diagnostic with a code, a path and a suggestion:

```json
{ "code": "unknown_action", "path": "/steps/0/action", "message": "Action 'http.requst' does not exist", "suggestion": "Did you mean 'http.request'?" }
```

The agent applies the suggestion with `replace_draft` and validates again until `valid` is `true`.

**3. Dry-run.** The agent calls `dry_run` on Version 1, mocking the Step `probe` (mocks are keyed by Step id), for example `{"probe": {"output": {"status": 200, "body": {"status": "up"}}}}`. The result has `"dry_run": true` and a Timeline whose entry shows the Step's `params` as written, `{{ secrets.API_TOKEN }}` included. Nothing is stored and the target is not called.

**4. Ask a human to publish.** The agent has no publish tool. If it tries the API with its Token it gets `403 insufficient_scope`; you can see the same refusal yourself:

```sh
BIA_TOKEN=<agent token> bia workflow publish uptime-check 1
# error[insufficient_scope]: this route needs an `admin` Token; this Token's Scope is `agent`
```

You review the Draft, then publish and activate it with the admin Token:

```sh
bia workflow publish uptime-check 1
bia workflow activate uptime-check 1
```

**5. Cron fires and the target fails.** Within a minute, a Run starts from the cron Trigger. The target answers `503`, so the Run fails: `bia run list --failed` shows it.

**6. Inspect and retry.** Ask: _"Which Runs of `uptime-check` failed, and why?"_ The agent calls `list_failures`, which lists each failed Run with the Timeline entry that failed it (`the server responded with status 503`, `Authorization` still shown as the Template). Make the target recover, then ask the agent to retry:

```sh
touch up        # in the directory where target.py runs
```

The agent calls `retry_run` with the failed Run's id. It creates a new Run, linked to the failed one by `retry_of`, which succeeds; `get_run` shows the target's `{"status": "up"}`. The failed Run stays as it was. The Secret's value appears nowhere in these Timelines.

When you are done, stop `target.py` and `bia serve` with Ctrl-C and `docker compose down`.

## Driving the tools by hand

Save this as `mcp.py`. It starts `bia mcp` (the same stdio server the clients use), calls one tool and prints its structured answer; without arguments it lists the tools.

```python
import json, subprocess, sys

tool = sys.argv[1] if len(sys.argv) > 1 else None
args = json.loads(sys.argv[2]) if len(sys.argv) > 2 else {}
p = subprocess.Popen(["bia", "mcp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)


def send(m):
    p.stdin.write(json.dumps(m) + "\n")
    p.stdin.flush()


def recv(i):
    while True:
        m = json.loads(p.stdout.readline())
        if m.get("id") == i:
            return m


send({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": "quickstart", "version": "0"}}})
recv(1)
send({"jsonrpc": "2.0", "method": "notifications/initialized"})
if tool is None:
    send({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})
    print([t["name"] for t in recv(2)["result"]["tools"]])
else:
    send({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": tool, "arguments": args}})
    r = recv(2)["result"]
    print(("ERROR " if r.get("isError") else "") + json.dumps(r.get("structuredContent"), indent=1))
p.stdin.close()
p.wait()
```

With `BIA_TOKEN` set to the agent Token, the whole scenario is:

```sh
cat > def.json <<'EOF'
{"schema_version": 1, "id": "uptime-check",
 "triggers": [{"id": "every_minute", "type": "cron", "expr": "* * * * *"}],
 "steps": [{"id": "probe", "action": "http.requst", "timeout": "10s",
   "params": {"method": "GET", "url": "http://127.0.0.1:9000/health",
              "headers": {"Authorization": "Bearer {{ secrets.API_TOKEN }}"}}}]}
EOF
args() { python3 -c 'import json,sys;print(json.dumps({"workflow_id":"uptime-check",**json.loads(sys.argv[1]),"definition":json.load(open("def.json"))}))' "$1"; }

python3 mcp.py create_draft "$(args '{}')"                               # step 1
python3 mcp.py validate_workflow '{"workflow_id":"uptime-check","number":1,"target":"publish"}'   # step 2: the Diagnostic
sed -i.bak 's/http.requst/http.request/' def.json
python3 mcp.py replace_draft "$(args '{"number":1}')"
python3 mcp.py validate_workflow '{"workflow_id":"uptime-check","number":1,"target":"publish"}'   # valid
python3 mcp.py dry_run '{"workflow_id":"uptime-check","number":1,"mocks":{"probe":{"output":{"status":200,"body":{"status":"up"}}}}}'   # step 3
# steps 4 and 5: refuse, then publish, as in the section above; wait a minute
python3 mcp.py list_failures                                             # step 6
touch up
python3 mcp.py retry_run '{"run_id":"<id from list_failures>"}'
python3 mcp.py list_runs '{"workflow_id":"uptime-check"}'
```

The first call saves the Draft with the typo. `create_draft` and `replace_draft` take the whole Definition, which `args` wraps with the Workflow id (and, for `replace_draft`, the Version number).

Without an MCP client at all, the same steps are `bia workflow apply -f def.json`, `bia workflow validate`, `bia workflow dry-run`, `bia run list --failed` and `bia run retry`; see `bia --help`.
