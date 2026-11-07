# Quickstart: the anchor scenario

This walks the scenario bia must support: an agent, over MCP, authors an uptime-check Workflow, fixes a Diagnostic and dry-runs it; a human approves its publication in the UI; cron runs it; the agent inspects the failed Runs and retries one. The automated version of the same scenario is `crates/bia-cli/tests/anchor_scenario.rs`.

You need Rust (the pinned toolchain installs itself), Docker, `openssl`, `python3`, and a clone of this repository. Run every command from the repository root unless a step says otherwise.

## 1. Install and start bia

```sh
cargo install --path crates/bia-cli --locked      # installs the `bia` binary
docker compose up --detach --wait postgres        # Postgres on localhost:5432

export DATABASE_URL=postgres://bia:bia@localhost:5432/bia
export BIA_MASTER_KEY="$(openssl rand -hex 32)"      # encrypts Secrets; back it up

bia migrate
bia serve --role all
```

`bia serve --role all` keeps running, so use a second terminal for the rest. The master key is only read by the server. On this fresh database the server logs a Setup Link, `http://localhost:8080/setup?code=…`: keep it for the next step. See [`configuration.md`](configuration.md) for every setting.

The quickstart uses the embedded UI, so build with it (`cargo install --path crates/bia-cli --locked --features ui`, after `just ui-build`; see [`deploy-ui.md`](deploy-ui.md)). Nothing below needs a Token to be copied by hand.

## 2. Set your password and sign in

Open the Setup Link in a browser and choose a password of at least 12 characters; setup signs you in to the UI. (Without a browser, `bia setup` asks for the link or its code and the password.) The Link only works until the next restart of `bia serve`.

Then sign the CLI in, in the second terminal. `BIA_URL` defaults to `http://127.0.0.1:8080`:

```sh
bia login
```

Your browser opens a consent screen for "bia CLI (built in)"; approve it. The CLI keeps the login in the OS keychain (or in `~/.config/bia/credentials.json`, mode 0600, when there is no keychain) and renews it on its own, so there is nothing to export. On a machine without a browser run `bia login --device`, which shows a code to enter at `{public_url}/device`. `bia logout` revokes it again. See [Access](configuration.md#access).

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

The URL is all an MCP client needs: `http://127.0.0.1:8080/mcp` (your `public_url` plus `/mcp`). The client finds bia's sign-in on its own (OAuth, ADR-0067), opens your browser, and you approve it on the consent screen as the `agent` Scope. It then shows under Connected apps in Access, where you can revoke it.

Claude Code:

```sh
claude mcp add --transport http bia http://127.0.0.1:8080/mcp
```

Then run `/mcp` in Claude Code, choose `bia` and authenticate.

Claude Desktop: Settings → Connectors → Add custom connector, with the URL `http://127.0.0.1:8080/mcp` and nothing else.

Cursor, in `~/.cursor/mcp.json`:

```json
{ "mcpServers": { "bia": { "url": "http://127.0.0.1:8080/mcp" } } }
```

For a client that cannot do OAuth but can run a command, `bia mcp` is a stdio proxy that uses the login from `bia login` (or `BIA_TOKEN`, if set): `claude mcp add bia --env BIA_URL=http://127.0.0.1:8080 -- bia mcp`. It acts with your Scope, which is `admin`, so prefer the URL form for agents.

The agent sees these tools: `get_schema`, `list_actions`, `list_workflows`, `get_workflow`, `list_versions`, `get_version`, `validate_workflow`, `create_draft`, `replace_draft`, `dry_run`, `diff_versions`, `trigger_run`, `get_run`, `list_runs`, `list_failures`, `retry_run`, `resume_run`, `cancel_run`, `list_secrets`, `list_queues`, `request_approval`, `get_approval_request` and `withdraw_approval_request`. There is deliberately no tool to publish, activate, approve or reject.

## 5. Walk the scenario

Each step lists what to ask the agent and, below it, the tool calls it makes. If you have no MCP client, [Driving the tools by hand](#driving-the-tools-by-hand) runs the same calls from a shell.

**1. Author a Draft.** Ask: _"Using bia, create a Workflow `uptime-check` that runs every minute, calls `http://127.0.0.1:9000/health` with the header `Authorization: Bearer {{ secrets.API_TOKEN }}`, and times out after 10 seconds."_ The agent calls `get_schema` and `list_actions`, then `create_draft`. Expect a Draft, Version 1. A typo such as `http.requst` is accepted in a Draft.

**2. Fix the Diagnostic.** The agent calls `validate_workflow` with `target: "publish"`. A typo comes back as a Diagnostic with a code, a path and a suggestion:

```json
{ "code": "unknown_action", "path": "/steps/0/action", "message": "Action 'http.requst' does not exist", "suggestion": "Did you mean 'http.request'?" }
```

The agent applies the suggestion with `replace_draft` and validates again until `valid` is `true`.

**3. Dry-run.** The agent calls `dry_run` on Version 1, mocking the Step `probe` (mocks are keyed by Step id), for example `{"probe": {"output": {"status": 200, "body": {"status": "up"}}}}`. The result has `"dry_run": true` and a Timeline whose entry shows the Step's `params` as written, `{{ secrets.API_TOKEN }}` included. Nothing is stored and the target is not called.

**4. Ask a human to publish.** The agent has no publish tool, and a publish call with its credential is refused with `403 insufficient_scope`. It calls `request_approval` with the Workflow, the Version, the action (`publish`, `publish_and_activate` or `activate`) and a note. The request must pass publishing's checks, or the call fails with the Diagnostics.

In the UI, the Approvals item in the navigation now shows a badge. Open the request, read the note and the Definition (or the diff against the Active Version, once there is one), and Approve. Approving publishes and activates Version 1 as you, in one step; Reject with a comment sends the agent back to the Draft. Replacing the Draft while the request is pending marks it `stale`, so you never approve content you did not see. From a terminal: `bia approval list`, `bia approval show <id>`, `bia approval approve <id>`.

The agent finds the decision with `get_approval_request` (with `request_id`), which carries your comment.

**5. Cron fires and the target fails.** Within a minute, a Run starts from the cron Trigger. The target answers `503`, so the Run fails: `bia run list --failed` shows it.

**6. Inspect and retry.** Ask: _"Which Runs of `uptime-check` failed, and why?"_ The agent calls `list_failures`, which lists each failed Run with the Timeline entry that failed it (`the server responded with status 503`, `Authorization` still shown as the Template). Make the target recover, then ask the agent to retry:

```sh
touch up        # in the directory where target.py runs
```

The agent calls `retry_run` with the failed Run's id. It creates a new Run, linked to the failed one by `retry_of`, which succeeds; `get_run` shows the target's `{"status": "up"}`. The failed Run stays as it was. The Secret's value appears nowhere in these Timelines.

When you are done, stop `target.py` and `bia serve` with Ctrl-C and `docker compose down`. `bia logout` removes the CLI's login, and you can revoke the MCP client in Access.

## Driving the tools by hand

Save this as `mcp.py`. It starts `bia mcp` (the stdio server, which uses your `bia login`), calls one tool and prints its structured answer; without arguments it lists the tools. It acts with your `admin` Scope, so the Scope refusals are not shown here.

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

With the login from step 2, the whole scenario is:

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
# step 4: python3 mcp.py request_approval ..., then approve in the UI or with `bia approval approve`; wait a minute
python3 mcp.py list_failures                                             # step 6
touch up
python3 mcp.py retry_run '{"run_id":"<id from list_failures>"}'
python3 mcp.py list_runs '{"workflow_id":"uptime-check"}'
```

The first call saves the Draft with the typo. `create_draft` and `replace_draft` take the whole Definition, which `args` wraps with the Workflow id (and, for `replace_draft`, the Version number).

Without an MCP client at all, the same steps are `bia workflow apply -f def.json`, `bia workflow validate`, `bia workflow dry-run`, `bia run list --failed` and `bia run retry`; see `bia --help`.
