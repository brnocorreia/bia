import type { GrantInfo } from "@/api/grants";
import { messageOf } from "@/api/request";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { formatTime } from "@/lib/format";
import { describeGrantClient, describeResource } from "@/oauth/provenance";

/**
 * The OAuth clients allowed to use the API for the admin, one row per Grant. Revoking a Grant
 * ends all of its access tokens and its refresh tokens.
 */
export function ConnectedAppsSection({
  grants,
  error,
  onRevoke,
}: {
  grants: GrantInfo[];
  error?: unknown;
  onRevoke: (grant: GrantInfo) => void;
}) {
  return (
    <section aria-labelledby="connected-apps-heading" className="mt-8">
      <h2 id="connected-apps-heading" className="text-lg font-semibold">
        Connected apps
      </h2>
      <p className="mt-1 text-sm text-muted-foreground">
        Apps such as MCP clients that you approved with OAuth. A client that is connected signs
        itself in again until you revoke it.
      </p>
      {error != null && (
        <p role="alert" className="mt-2 text-sm text-destructive">
          {messageOf(error)}
        </p>
      )}
      {grants.length === 0 ? (
        <p className="mt-3 text-sm text-muted-foreground">No connected apps.</p>
      ) : (
        <table className="mt-3 w-full text-left text-sm">
          <thead className="border-b text-muted-foreground">
            <tr>
              <th className="py-2 pr-4 font-medium">App</th>
              <th className="py-2 pr-4 font-medium">Scope</th>
              <th className="py-2 pr-4 font-medium">Connected</th>
              <th className="py-2 pr-4 font-medium">Last used</th>
              <th className="py-2 pr-4 font-medium">Status</th>
              <th className="py-2 font-medium">
                <span className="sr-only">Actions</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {grants.map((grant) => (
              <tr key={grant.id} className="border-b">
                <td className="py-2 pr-4" title={describeResource(grant.resource)}>
                  <span className="font-medium">{grant.client_name}</span>
                  <span className="block text-xs text-muted-foreground">
                    {describeGrantClient(grant)}
                  </span>
                </td>
                <td className="py-2 pr-4">
                  <Badge variant={grant.scope === "admin" ? "destructive" : "secondary"}>
                    {grant.scope}
                  </Badge>
                </td>
                <td className="py-2 pr-4">{formatTime(grant.created_at)}</td>
                <td className="py-2 pr-4">{formatTime(grant.last_used_at)}</td>
                <td className="py-2 pr-4">
                  {grant.revoked_at ? (
                    <Badge variant="outline">Revoked {formatTime(grant.revoked_at)}</Badge>
                  ) : (
                    "Active"
                  )}
                </td>
                <td className="py-2 text-right">
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={grant.revoked_at != null}
                    aria-label={`Revoke ${grant.client_name}`}
                    onClick={() => onRevoke(grant)}
                  >
                    Revoke
                  </Button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}
