import { createFileRoute, redirect } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import {
  approveConsent,
  denyConsent,
  getConsent,
  type ConsentRequest,
  type ScopeName,
} from "@/api/grants";
import { ApiError, messageOf } from "@/api/request";
import { currentTokenQuery } from "@/auth/scope";
import { session } from "@/auth/session";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { describeProvenance, describeResource } from "@/oauth/provenance";

interface Search {
  request?: string;
}

// Where an app's authorization request lands the signed-in person. It sits outside the app
// shell: this is a decision about one app, not a place to browse from.
export const Route = createFileRoute("/oauth/consent")({
  validateSearch: (search: Record<string, unknown>): Search => ({
    request: typeof search.request === "string" && search.request ? search.request : undefined,
  }),
  beforeLoad: async ({ context, location }) => {
    if (session.getState() === "signed-in") return;
    try {
      await context.queryClient.fetchQuery(currentTokenQuery);
      session.signedIn();
    } catch {
      throw redirect({ to: "/login", search: { redirect: location.href } });
    }
  },
  component: Consent,
});

function Consent() {
  const { request } = Route.useSearch();
  const pending = useQuery({
    queryKey: ["oauth-consent", request],
    queryFn: () => getConsent(request!),
    enabled: request !== undefined,
    retry: false,
    // The request is single-use and short-lived; never show a stale copy.
    gcTime: 0,
  });

  return (
    <main className="flex min-h-screen items-center justify-center p-4">
      <Card className="w-full max-w-lg">
        {request === undefined ? (
          <Problem message="This link has no authorization request. Start again from the app." />
        ) : pending.isPending ? (
          <CardContent className="p-6 text-sm text-muted-foreground">Loading…</CardContent>
        ) : pending.error ? (
          <Problem
            message={
              pending.error instanceof ApiError && pending.error.code === "authorization_not_found"
                ? "This request was already answered or has run out. Start again from the app."
                : messageOf(pending.error)
            }
          />
        ) : (
          <Decide request={request} consent={pending.data} />
        )}
      </Card>
    </main>
  );
}

function Problem({ message }: { message: string }) {
  return (
    <>
      <CardHeader>
        <CardTitle>Cannot ask you</CardTitle>
      </CardHeader>
      <CardContent>
        <p role="alert" className="text-sm text-destructive">
          {message}
        </p>
      </CardContent>
    </>
  );
}

const SCOPE_TEXT: Record<ScopeName, string> = {
  agent: "Agent: author Drafts, validate, dry-run, trigger and read. Cannot publish or activate.",
  admin: "Admin: everything, including publishing, activating and managing Secrets and access.",
};

function Decide({ request, consent }: { request: string; consent: ConsentRequest }) {
  const [scope, setScope] = useState<ScopeName>(consent.requested_scope);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const { client } = consent;

  async function answer(decide: () => Promise<string>) {
    setBusy(true);
    setError(null);
    try {
      // The server built this address from the redirect URI it recorded for the app.
      window.location.assign(await decide());
    } catch (failure) {
      setBusy(false);
      setError(messageOf(failure));
    }
  }

  const scopes: ScopeName[] = consent.requested_scope === "admin" ? ["admin", "agent"] : ["agent"];

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
          <dt className="text-muted-foreground">Sends you back to</dt>
          <dd className="break-all font-mono">{consent.redirect_uri}</dd>
          <dt className="text-muted-foreground">Access to</dt>
          <dd>{describeResource(consent.resource)}</dd>
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
          <Button
            variant="outline"
            disabled={busy}
            onClick={() => answer(() => denyConsent(request))}
          >
            Deny
          </Button>
          <Button disabled={busy} onClick={() => answer(() => approveConsent(request, scope))}>
            Approve
          </Button>
        </div>
      </CardContent>
    </>
  );
}
