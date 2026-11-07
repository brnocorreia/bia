import { redirect } from "@tanstack/react-router";
import type { QueryClient } from "@tanstack/react-query";
import { currentTokenQuery } from "./scope";
import { session } from "./session";

/** For a route outside the app shell that needs the person signed in: sends them to sign in,
 *  and back to `location` afterwards. */
export async function requireSession(queryClient: QueryClient, location: { href: string }) {
  if (session.getState() === "signed-in") return;
  try {
    await queryClient.fetchQuery(currentTokenQuery);
    session.signedIn();
  } catch {
    throw redirect({ to: "/login", search: { redirect: location.href } });
  }
}
