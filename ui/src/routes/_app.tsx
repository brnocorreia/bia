import {
  createFileRoute,
  Link,
  Outlet,
  redirect,
  useNavigate,
  useRouter,
} from "@tanstack/react-router";
import { useEffect } from "react";
import { currentTokenQuery } from "@/auth/scope";
import { session, useSessionState } from "@/auth/session";
import { UserMenu } from "@/components/user-menu";
import { cn } from "@/lib/utils";

// Layout of every signed-in screen. Screens are sibling files named `_app.<name>.tsx`; one
// that sets `staticData: { nav }` gets a navigation entry without touching this file.
export const Route = createFileRoute("/_app")({
  beforeLoad: async ({ context }) => {
    if (session.getState() === "signed-in") return;
    // A reload forgets what the UI knew; the cookie is still there if the Session is.
    try {
      await context.queryClient.fetchQuery(currentTokenQuery);
      session.signedIn();
    } catch {
      throw redirect({ to: "/login" });
    }
  },
  component: Shell,
});

function Shell() {
  const router = useRouter();
  const navigate = useNavigate();
  const state = useSessionState();

  // A 401 anywhere ends the session; this is the one place that sends the user back.
  useEffect(() => {
    if (state === "signed-out") void navigate({ to: "/login" });
  }, [state, navigate]);

  const entries = Object.values(router.routesByPath)
    .flatMap((route) => {
      const nav = route.options.staticData?.nav;
      return nav ? [{ ...nav, to: route.fullPath }] : [];
    })
    .sort((a, b) => a.order - b.order);

  if (state === "signed-out") return null;

  return (
    <div className="flex min-h-screen">
      <aside className="flex w-56 shrink-0 flex-col gap-6 border-r p-4">
        <span className="text-lg font-semibold">bia</span>
        <nav aria-label="Main" className="flex flex-1 flex-col gap-1">
          {entries.map((entry) => (
            <Link
              key={entry.to}
              to={entry.to}
              className={cn("rounded-md px-3 py-2 text-sm hover:bg-accent")}
              activeProps={{ className: "bg-accent font-medium" }}
            >
              {entry.label}
            </Link>
          ))}
        </nav>
        <UserMenu />
      </aside>
      <main className="flex-1 p-6">
        <Outlet />
      </main>
    </div>
  );
}
