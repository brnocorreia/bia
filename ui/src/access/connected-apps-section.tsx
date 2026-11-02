import type { TokenInfo } from "@/api/tokens";

/**
 * The OAuth clients allowed to use the API for the admin, one row per Grant. Placeholder until
 * Grants exist (BIA-95), which replaces this file: `oauth` is the `oauth` Tokens, which carry
 * a `grant_id` to group by, and `onRevoke` is the screen's revoke flow.
 */
export function ConnectedAppsSection(_props: {
  oauth: TokenInfo[];
  onRevoke: (token: TokenInfo) => void;
}) {
  return (
    <section aria-labelledby="connected-apps-heading" className="mt-8">
      <h2 id="connected-apps-heading" className="text-lg font-semibold">
        Connected apps
      </h2>
      <p className="mt-1 text-sm text-muted-foreground">
        Apps such as MCP clients will appear here once they can ask for access with OAuth. None can
        yet.
      </p>
    </section>
  );
}
