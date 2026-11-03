import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useRevokeGrant, type GrantInfo } from "@/api/grants";
import { useRevokeToken, type TokenInfo } from "@/api/tokens";
import { messageOf } from "@/api/request";
import { currentTokenQuery } from "@/auth/scope";
import { session } from "@/auth/session";
import { ConfirmDialog } from "@/components/confirm-dialog";

type Target = { kind: "token"; token: TokenInfo } | { kind: "grant"; grant: GrantInfo };

/**
 * Asks before revoking any credential or connected app. Revoking the Session this browser is
 * using signs it out, so the UI goes back to the sign-in screen at once rather than on its next
 * failed request.
 */
export function useRevokeFlow(describe: (token: TokenInfo) => string) {
  const revokeToken = useRevokeToken();
  const revokeGrant = useRevokeGrant();
  const current = useQuery(currentTokenQuery);
  const [revoking, setRevoking] = useState<Target | null>(null);
  const isOwnSession = revoking?.kind === "token" && revoking.token.id === current.data?.id;
  const mutation = revoking?.kind === "grant" ? revokeGrant : revokeToken;

  const title =
    revoking?.kind === "grant"
      ? revoking.grant.client_name
      : revoking
        ? describe(revoking.token)
        : "credential";

  const dialog = (
    <ConfirmDialog
      open={revoking !== null}
      title={`Revoke ${title}?`}
      confirmLabel="Revoke"
      busy={mutation.isPending}
      error={mutation.error ? messageOf(mutation.error) : undefined}
      onCancel={() => setRevoking(null)}
      onConfirm={() => {
        if (!revoking) return;
        const done = {
          onSuccess: () => {
            setRevoking(null);
            if (isOwnSession) session.signedOut();
          },
        };
        if (revoking.kind === "grant") revokeGrant.mutate(revoking.grant.id, done);
        else revokeToken.mutate(revoking.token.id, done);
      }}
    >
      {isOwnSession
        ? "This is the Session you are using, so you will be signed out."
        : revoking?.kind === "grant"
          ? "Its access tokens and refresh token stop working at once, so the app has to ask you again. This cannot be undone."
          : "It stops authenticating at once and cannot be restored. Anything using it loses access."}
    </ConfirmDialog>
  );

  const ask = (target: Target) => {
    revokeToken.reset();
    revokeGrant.reset();
    setRevoking(target);
  };

  return {
    ask: (token: TokenInfo) => ask({ kind: "token", token }),
    askGrant: (grant: GrantInfo) => ask({ kind: "grant", grant }),
    dialog,
  };
}
