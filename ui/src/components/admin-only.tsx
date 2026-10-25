import type { ReactNode } from "react";
import { useCurrentToken } from "@/auth/scope";

/** Renders `children` for an admin Token and `fallback` (nothing by default) otherwise. */
export function AdminOnly({
  children,
  fallback = null,
}: {
  children: ReactNode;
  fallback?: ReactNode;
}) {
  return useCurrentToken().data?.scope === "admin" ? children : fallback;
}

/**
 * A whole screen that only an admin Token may use (Secrets, Tokens). Shows an explanation
 * instead of the screen for any other Scope, and nothing while the Scope loads.
 */
export function RequireAdmin({ children, what }: { children: ReactNode; what: string }) {
  const token = useCurrentToken();
  if (token.data?.scope === "admin") return children;
  if (token.isPending) return null;
  return (
    <p role="alert" className="text-sm text-muted-foreground">
      {what} needs an admin Token. Sign out and sign in with one.
    </p>
  );
}
