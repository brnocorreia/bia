import { useRef, useState, type FormEvent } from "react";
import { createFileRoute } from "@tanstack/react-router";
import { useQueryClient } from "@tanstack/react-query";
import { putSecret, secretKeys, useDeleteSecret, useSecrets } from "@/api/secrets";
import { messageOf } from "@/api/request";
import { RequireAdmin } from "@/components/admin-only";
import { ConfirmDialog } from "@/components/confirm-dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { formatTime } from "@/lib/format";

export const Route = createFileRoute("/_app/secrets")({
  staticData: { nav: { label: "Secrets", order: 30 } },
  component: () => (
    <RequireAdmin what="Secrets">
      <Secrets />
    </RequireAdmin>
  ),
});

function Secrets() {
  const secrets = useSecrets();
  const [deleting, setDeleting] = useState<string | null>(null);
  const remove = useDeleteSecret();
  return (
    <section>
      <h1 className="text-2xl font-semibold">Secrets</h1>
      <p className="mt-1 text-sm text-muted-foreground">
        Values are write-only: set or replace one here, but it is never shown again. Steps read a
        Secret as <code>secrets.NAME</code>.
      </p>
      <SecretForm />
      {secrets.isPending && <p className="mt-4 text-sm text-muted-foreground">Loading…</p>}
      {secrets.error && (
        <p role="alert" className="mt-4 text-sm text-destructive">
          {messageOf(secrets.error)}
        </p>
      )}
      {secrets.data?.length === 0 && (
        <p className="mt-4 text-sm text-muted-foreground">No Secrets yet.</p>
      )}
      {secrets.data && secrets.data.length > 0 && (
        <table className="mt-4 w-full text-left text-sm">
          <thead className="border-b text-muted-foreground">
            <tr>
              <th className="py-2 pr-4 font-medium">Name</th>
              <th className="py-2 pr-4 font-medium">Created</th>
              <th className="py-2 pr-4 font-medium">Value last set</th>
              <th className="py-2 font-medium">
                <span className="sr-only">Actions</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {secrets.data.map((secret) => (
              <tr key={secret.name} className="border-b">
                <td className="py-2 pr-4 font-mono">{secret.name}</td>
                <td className="py-2 pr-4">{formatTime(secret.created_at)}</td>
                <td className="py-2 pr-4">{formatTime(secret.updated_at)}</td>
                <td className="py-2 text-right">
                  <Button
                    variant="outline"
                    size="sm"
                    aria-label={`Delete ${secret.name}`}
                    onClick={() => {
                      remove.reset();
                      setDeleting(secret.name);
                    }}
                  >
                    Delete
                  </Button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      <ConfirmDialog
        open={deleting !== null}
        title={`Delete ${deleting ?? "Secret"}?`}
        confirmLabel="Delete Secret"
        busy={remove.isPending}
        error={remove.error ? messageOf(remove.error) : undefined}
        onCancel={() => setDeleting(null)}
        onConfirm={() =>
          deleting && remove.mutate(deleting, { onSuccess: () => setDeleting(null) })
        }
      >
        Steps that read it will fail with <code>unknown_secret</code>. The value cannot be
        recovered.
      </ConfirmDialog>
    </section>
  );
}

// The value is read from an uncontrolled input and cleared as soon as it is sent, so it never
// enters React state or the query cache (ADR-0017). Naming an existing Secret replaces its value.
function SecretForm() {
  const queryClient = useQueryClient();
  const valueRef = useRef<HTMLInputElement>(null);
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState<string | null>(null);

  async function submit(event: FormEvent) {
    event.preventDefault();
    const input = valueRef.current;
    if (!input) return;
    const value = input.value;
    setBusy(true);
    setError(null);
    setSaved(null);
    try {
      const secret = await putSecret(name.trim(), value);
      input.value = "";
      setName("");
      setSaved(secret.name);
      await queryClient.invalidateQueries({ queryKey: secretKeys.all });
    } catch (e) {
      input.value = "";
      setError(messageOf(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <form onSubmit={submit} autoComplete="off" className="mt-6 flex flex-wrap items-end gap-3">
      <div className="space-y-1">
        <Label htmlFor="secret-name">Name</Label>
        <Input
          id="secret-name"
          value={name}
          onChange={(e) => setName(e.target.value)}
          autoComplete="off"
          spellCheck={false}
          placeholder="API_TOKEN"
          required
        />
      </div>
      <div className="space-y-1">
        <Label htmlFor="secret-value">Value</Label>
        <Input
          id="secret-value"
          ref={valueRef}
          type="password"
          name="secret-value"
          autoComplete="new-password"
          data-1p-ignore
          data-lpignore="true"
          spellCheck={false}
          required
        />
      </div>
      <Button type="submit" disabled={busy || name.trim() === ""}>
        {busy ? "Saving…" : "Set Secret"}
      </Button>
      <div className="basis-full text-sm" aria-live="polite">
        {saved && <span>Saved {saved}. Its value is not shown.</span>}
        {error && (
          <span role="alert" className="text-destructive">
            {error}
          </span>
        )}
      </div>
    </form>
  );
}
