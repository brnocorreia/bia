import { useState } from "react";
import type { ScopeName } from "@/api/grants";
import { messageOf } from "@/api/request";
import { Button } from "@/components/ui/button";
import { CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { describeProvenance, describeResource } from "@/oauth/provenance";

export interface ConsentClient {
  name: string;
  provenance: "cimd" | "dynamic" | "builtin";
  host?: string | null;
  verified: boolean;
}

const SCOPE_TEXT: Record<ScopeName, string> = {
  agent: "Agent: author Drafts, validate, dry-run, trigger and read. Cannot publish or activate.",
  admin: "Admin: everything, including publishing, activating and managing Secrets and access.",
};

interface Props {
  client: ConsentClient;
  requestedScope: ScopeName;
  resource: string;
  /** Where an authorization code flow sends the person back to; a device flow has none. */
  redirectUri?: string;
  /** Answers `approve` with the Scope chosen, which is at most what was requested. */
  onApprove: (scope: ScopeName) => Promise<void>;
  onDeny: () => Promise<void>;
}

/** What a person is asked when an app wants access, shared by the authorization code flow's
 *  consent screen and the device flow's. */
export function ConsentCard({
  client,
  requestedScope,
  resource,
  redirectUri,
  onApprove,
  onDeny,
}: Props) {
  const [scope, setScope] = useState<ScopeName>(requestedScope);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function answer(decide: () => Promise<void>) {
    setBusy(true);
    setError(null);
    try {
      await decide();
    } catch (failure) {
      setBusy(false);
      setError(messageOf(failure));
    }
  }

  const scopes: ScopeName[] = requestedScope === "admin" ? ["admin", "agent"] : ["agent"];

  return (
    <>
      <CardHeader>
        <CardTitle>Let {client.name} use bia?</CardTitle>
        <CardDescription>
          {client.name} is {describeProvenance(client)}.
        </CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-4">
        {!client.verified && (
          <p role="note" className="rounded-md border border-destructive p-3 text-sm">
            This app registered itself, so nothing vouches for its name. Only approve it if you just
            started connecting it yourself.
          </p>
        )}
        <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-2 text-sm">
          {redirectUri && (
            <>
              <dt className="text-muted-foreground">Sends you back to</dt>
              <dd className="break-all font-mono">{redirectUri}</dd>
            </>
          )}
          <dt className="text-muted-foreground">Access to</dt>
          <dd>{describeResource(resource)}</dd>
        </dl>
        <fieldset className="flex flex-col gap-2">
          <legend className="text-sm font-medium">
            {scopes.length > 1 ? "It asks for admin access. Allow:" : "It will get:"}
          </legend>
          {scopes.map((option) => (
            <label key={option} className="flex items-start gap-2 text-sm">
              <input
                type="radio"
                name="scope"
                value={option}
                checked={scope === option}
                disabled={scopes.length === 1}
                onChange={() => setScope(option)}
                className="mt-1"
              />
              <span>{SCOPE_TEXT[option]}</span>
            </label>
          ))}
        </fieldset>
        {error && (
          <p role="alert" className="text-sm text-destructive">
            {error}
          </p>
        )}
        <div className="flex justify-end gap-2">
          <Button variant="outline" disabled={busy} onClick={() => answer(onDeny)}>
            Deny
          </Button>
          <Button disabled={busy} onClick={() => answer(() => onApprove(scope))}>
            Approve
          </Button>
        </div>
      </CardContent>
    </>
  );
}
