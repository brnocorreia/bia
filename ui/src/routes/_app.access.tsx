import { createFileRoute } from "@tanstack/react-router";
import { useGrants } from "@/api/grants";
import { useTokens } from "@/api/tokens";
import { messageOf } from "@/api/request";
import { RequireAdmin } from "@/components/admin-only";
import { ConnectedAppsSection } from "@/access/connected-apps-section";
import { groupByOrigin } from "@/access/credentials";
import { useRevokeFlow } from "@/access/revoke-dialog";
import { SessionsSection } from "@/access/sessions-section";
import { TokensSection } from "@/access/tokens-section";

export const Route = createFileRoute("/_app/access")({
  staticData: { nav: { label: "Access", order: 40 } },
  component: () => (
    <RequireAdmin what="Access">
      <Access />
    </RequireAdmin>
  ),
});

function Access() {
  const tokens = useTokens();
  const grants = useGrants();
  const revoke = useRevokeFlow((token) =>
    token.origin === "session" ? "this Session" : token.name,
  );
  const credentials = tokens.data && groupByOrigin(tokens.data);
  return (
    <section>
      <h1 className="text-2xl font-semibold">Access</h1>
      <p className="mt-1 text-sm text-muted-foreground">
        Everything that can use this API, and a way to revoke each of it.
      </p>
      {tokens.isPending && <p className="mt-4 text-sm text-muted-foreground">Loading…</p>}
      {tokens.error && (
        <p role="alert" className="mt-4 text-sm text-destructive">
          {messageOf(tokens.error)}
        </p>
      )}
      {credentials && (
        <>
          <SessionsSection sessions={credentials.sessions} onRevoke={revoke.ask} />
          <ConnectedAppsSection
            grants={grants.data ?? []}
            error={grants.error}
            onRevoke={revoke.askGrant}
          />
          <TokensSection tokens={credentials.manual} onRevoke={revoke.ask} />
        </>
      )}
      {revoke.dialog}
    </section>
  );
}
