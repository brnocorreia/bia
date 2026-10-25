import { useState, type FormEvent } from "react";
import { createFileRoute } from "@tanstack/react-router";
import { useQueryClient } from "@tanstack/react-query";
import {
  createToken,
  tokenKeys,
  useRevokeToken,
  useTokens,
  type CreatedToken,
  type TokenInfo,
} from "@/api/tokens";
import { messageOf } from "@/api/request";
import { RequireAdmin } from "@/components/admin-only";
import { ConfirmDialog } from "@/components/confirm-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { formatTime } from "@/lib/format";
import type { Scope } from "@/auth/scope";

export const Route = createFileRoute("/_app/tokens")({
  staticData: { nav: { label: "Tokens", order: 40 } },
  component: () => (
    <RequireAdmin what="Tokens">
      <Tokens />
    </RequireAdmin>
  ),
});

function Tokens() {
  const tokens = useTokens();
  const revoke = useRevokeToken();
  const [revoking, setRevoking] = useState<TokenInfo | null>(null);
  return (
    <section>
      <h1 className="text-2xl font-semibold">Tokens</h1>
      <p className="mt-1 text-sm text-muted-foreground">
        A Token's value is shown once, when it is created. Give an <code>agent</code> Token to an
        agent to let it author Workflows.
      </p>
      <NewTokenForm />
      {tokens.isPending && <p className="mt-4 text-sm text-muted-foreground">Loading…</p>}
      {tokens.error && (
        <p role="alert" className="mt-4 text-sm text-destructive">
          {messageOf(tokens.error)}
        </p>
      )}
      {tokens.data && (
        <table className="mt-4 w-full text-left text-sm">
          <thead className="border-b text-muted-foreground">
            <tr>
              <th className="py-2 pr-4 font-medium">Name</th>
              <th className="py-2 pr-4 font-medium">Scope</th>
              <th className="py-2 pr-4 font-medium">Created</th>
              <th className="py-2 pr-4 font-medium">Last used</th>
              <th className="py-2 pr-4 font-medium">Status</th>
              <th className="py-2 font-medium">
                <span className="sr-only">Actions</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {tokens.data.map((token) => (
              <tr key={token.id} className="border-b">
                <td className="py-2 pr-4">{token.name}</td>
                <td className="py-2 pr-4">
                  <Badge variant="secondary">{token.scope}</Badge>
                  {token.webhook && (
                    <span className="ml-2 text-muted-foreground">
                      {token.webhook.workflow_id} / {token.webhook.trigger_id}
                    </span>
                  )}
                </td>
                <td className="py-2 pr-4">{formatTime(token.created_at)}</td>
                <td className="py-2 pr-4">{formatTime(token.last_used_at)}</td>
                <td className="py-2 pr-4">
                  {token.revoked_at ? (
                    <Badge variant="outline">Revoked {formatTime(token.revoked_at)}</Badge>
                  ) : (
                    "Active"
                  )}
                </td>
                <td className="py-2 text-right">
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={token.revoked_at != null}
                    aria-label={`Revoke ${token.name}`}
                    onClick={() => {
                      revoke.reset();
                      setRevoking(token);
                    }}
                  >
                    Revoke
                  </Button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      <ConfirmDialog
        open={revoking !== null}
        title={`Revoke ${revoking?.name ?? "Token"}?`}
        confirmLabel="Revoke Token"
        busy={revoke.isPending}
        error={revoke.error ? messageOf(revoke.error) : undefined}
        onCancel={() => setRevoking(null)}
        onConfirm={() =>
          revoking && revoke.mutate(revoking.id, { onSuccess: () => setRevoking(null) })
        }
      >
        It stops authenticating at once and cannot be restored. Anything using it loses access.
      </ConfirmDialog>
    </section>
  );
}

const SCOPES: Scope[] = ["agent", "webhook", "admin"];

// The created Token lives only in this component's state: dismissing it, leaving the screen
// or reloading drops it, and nothing else holds the value (it is not in the query cache).
function NewTokenForm() {
  const queryClient = useQueryClient();
  const [name, setName] = useState("");
  const [scope, setScope] = useState<Scope>("agent");
  const [workflowId, setWorkflowId] = useState("");
  const [triggerId, setTriggerId] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [created, setCreated] = useState<CreatedToken | null>(null);

  async function submit(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const result = await createToken({
        name: name.trim(),
        scope,
        webhook:
          scope === "webhook"
            ? { workflow_id: workflowId.trim(), trigger_id: triggerId.trim() }
            : null,
      });
      setCreated(result);
      setName("");
      setWorkflowId("");
      setTriggerId("");
      await queryClient.invalidateQueries({ queryKey: tokenKeys.all });
    } catch (e) {
      setError(messageOf(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="mt-6">
      <form onSubmit={submit} autoComplete="off" className="flex flex-wrap items-end gap-3">
        <div className="space-y-1">
          <Label htmlFor="token-name">Name</Label>
          <Input
            id="token-name"
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="claude-desktop"
            required
          />
        </div>
        <div className="space-y-1">
          <Label htmlFor="token-scope">Scope</Label>
          <select
            id="token-scope"
            value={scope}
            onChange={(e) => setScope(e.target.value as Scope)}
            className="flex h-9 rounded-md border border-input bg-transparent px-3 py-1 text-sm shadow-xs"
          >
            {SCOPES.map((s) => (
              <option key={s} value={s}>
                {s}
              </option>
            ))}
          </select>
        </div>
        {scope === "webhook" && (
          <>
            <div className="space-y-1">
              <Label htmlFor="token-workflow">Workflow</Label>
              <Input
                id="token-workflow"
                value={workflowId}
                onChange={(e) => setWorkflowId(e.target.value)}
                required
              />
            </div>
            <div className="space-y-1">
              <Label htmlFor="token-trigger">Trigger</Label>
              <Input
                id="token-trigger"
                value={triggerId}
                onChange={(e) => setTriggerId(e.target.value)}
                required
              />
            </div>
          </>
        )}
        <Button type="submit" disabled={busy || name.trim() === ""}>
          {busy ? "Creating…" : "Create Token"}
        </Button>
      </form>
      {error && (
        <p role="alert" className="mt-2 text-sm text-destructive">
          {error}
        </p>
      )}
      {created && (
        <div role="status" className="mt-4 rounded-md border bg-card p-4 text-sm">
          <p className="font-medium">
            Token {created.token.name} created. Copy its value now: it is shown only once.
          </p>
          <div className="mt-2 flex items-center gap-2">
            <code
              data-testid="new-token-value"
              className="break-all rounded bg-muted px-2 py-1 font-mono"
            >
              {created.value}
            </code>
            <Button
              variant="outline"
              size="sm"
              onClick={() => void navigator.clipboard?.writeText(created.value)}
            >
              Copy
            </Button>
            <Button variant="outline" size="sm" onClick={() => setCreated(null)}>
              Done
            </Button>
          </div>
        </div>
      )}
    </div>
  );
}
