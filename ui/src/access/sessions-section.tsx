import { useQuery } from "@tanstack/react-query";
import { useSignOutOthers, type TokenInfo } from "@/api/tokens";
import { messageOf } from "@/api/request";
import { currentTokenQuery } from "@/auth/scope";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { formatTime } from "@/lib/format";
import { describeBrowser } from "./credentials";

export function SessionsSection({
  sessions,
  onRevoke,
}: {
  sessions: TokenInfo[];
  onRevoke: (token: TokenInfo) => void;
}) {
  const current = useQuery(currentTokenQuery);
  const signOutOthers = useSignOutOthers();
  const hasOthers = sessions.some((s) => s.revoked_at == null && s.id !== current.data?.id);

  return (
    <section aria-labelledby="sessions-heading" className="mt-8">
      <div className="flex items-center justify-between">
        <h2 id="sessions-heading" className="text-lg font-semibold">
          Sessions
        </h2>
        <Button
          variant="outline"
          size="sm"
          disabled={!hasOthers || signOutOthers.isPending}
          onClick={() => signOutOthers.mutate()}
        >
          Sign out other sessions
        </Button>
      </div>
      <p className="mt-1 text-sm text-muted-foreground">
        Browsers signed in with the password. A Session ends after 14 days unused.
      </p>
      {signOutOthers.error && (
        <p role="alert" className="mt-2 text-sm text-destructive">
          {messageOf(signOutOthers.error)}
        </p>
      )}
      {sessions.length === 0 ? (
        <p className="mt-3 text-sm text-muted-foreground">No Sessions.</p>
      ) : (
        <table className="mt-3 w-full text-left text-sm">
          <thead className="border-b text-muted-foreground">
            <tr>
              <th className="py-2 pr-4 font-medium">Browser</th>
              <th className="py-2 pr-4 font-medium">Signed in</th>
              <th className="py-2 pr-4 font-medium">Last used</th>
              <th className="py-2 pr-4 font-medium">Status</th>
              <th className="py-2 font-medium">
                <span className="sr-only">Actions</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {sessions.map((s) => {
              const label = describeBrowser(s.user_agent);
              return (
                <tr key={s.id} className="border-b">
                  <td className="py-2 pr-4" title={s.user_agent ?? undefined}>
                    {label}
                    {s.id === current.data?.id && (
                      <Badge variant="secondary" className="ml-2">
                        This device
                      </Badge>
                    )}
                  </td>
                  <td className="py-2 pr-4">{formatTime(s.created_at)}</td>
                  <td className="py-2 pr-4">{formatTime(s.last_used_at)}</td>
                  <td className="py-2 pr-4">
                    {s.revoked_at ? (
                      <Badge variant="outline">Revoked {formatTime(s.revoked_at)}</Badge>
                    ) : (
                      "Active"
                    )}
                  </td>
                  <td className="py-2 text-right">
                    <Button
                      variant="outline"
                      size="sm"
                      disabled={s.revoked_at != null}
                      aria-label={`Revoke session ${label}`}
                      onClick={() => onRevoke(s)}
                    >
                      Revoke
                    </Button>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}
    </section>
  );
}
