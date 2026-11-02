import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useRevokeToken, type TokenInfo } from "@/api/tokens";
import { messageOf } from "@/api/request";
import { currentTokenQuery } from "@/auth/scope";
import { session } from "@/auth/session";
import { ConfirmDialog } from "@/components/confirm-dialog";

/**
 * Asks before revoking any credential. Revoking the Session this browser is using signs it out,
 * so the UI goes back to the sign-in screen at once rather than on its next failed request.
 */
export function useRevokeFlow(describe: (token: TokenInfo) => string) {
  const revoke = useRevokeToken();
  const current = useQuery(currentTokenQuery);
  const [revoking, setRevoking] = useState<TokenInfo | null>(null);
  const isOwnSession = revoking != null && revoking.id === current.data?.id;

  const dialog = (
    <ConfirmDialog
      open={revoking !== null}
      title={`Revoke ${revoking ? describe(revoking) : "credential"}?`}
      confirmLabel="Revoke"
      busy={revoke.isPending}
      error={revoke.error ? messageOf(revoke.error) : undefined}
      onCancel={() => setRevoking(null)}
      onConfirm={() =>
        revoking &&
        revoke.mutate(revoking.id, {
          onSuccess: () => {
            setRevoking(null);
            if (isOwnSession) session.signedOut();
          },
        })
      }
    >
      {isOwnSession
        ? "This is the Session you are using, so you will be signed out."
        : "It stops authenticating at once and cannot be restored. Anything using it loses access."}
    </ConfirmDialog>
  );

  return {
    ask: (token: TokenInfo) => {
      revoke.reset();
      setRevoking(token);
    },
    dialog,
  };
}
