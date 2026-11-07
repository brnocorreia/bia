import { createFileRoute } from "@tanstack/react-router";
import { useQuery } from "@tanstack/react-query";
import { approveConsent, denyConsent, getConsent, type ConsentRequest } from "@/api/grants";
import { ApiError, messageOf } from "@/api/request";
import { requireSession } from "@/auth/require-session";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { ConsentCard } from "@/oauth/consent-card";

interface Search {
  request?: string;
}

// Where an app's authorization request lands the signed-in person. It sits outside the app
// shell: this is a decision about one app, not a place to browse from.
export const Route = createFileRoute("/oauth/consent")({
  validateSearch: (search: Record<string, unknown>): Search => ({
    request: typeof search.request === "string" && search.request ? search.request : undefined,
  }),
  beforeLoad: ({ context, location }) => requireSession(context.queryClient, location),
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

function Decide({ request, consent }: { request: string; consent: ConsentRequest }) {
  // The server built these addresses from the redirect URI it recorded for the app.
  return (
    <ConsentCard
      client={consent.client}
      requestedScope={consent.requested_scope}
      resource={consent.resource}
      redirectUri={consent.redirect_uri}
      onApprove={async (scope) => window.location.assign(await approveConsent(request, scope))}
      onDeny={async () => window.location.assign(await denyConsent(request))}
    />
  );
}
