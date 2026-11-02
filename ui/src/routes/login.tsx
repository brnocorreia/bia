import { createFileRoute, redirect, useNavigate } from "@tanstack/react-router";
import { useState, type FormEvent } from "react";
import { signIn, signInWithPasskey } from "@/auth/login";
import { PASSKEYS_NEED_HTTPS, passkeysSupported } from "@/auth/webauthn";
import { session } from "@/auth/session";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

export const Route = createFileRoute("/login")({
  beforeLoad: () => {
    if (session.getState() === "signed-in") throw redirect({ to: "/" });
  },
  component: Login,
});

function Login() {
  const navigate = useNavigate();
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const supported = passkeysSupported();

  async function submit(event: FormEvent) {
    event.preventDefault();
    await start(() => signIn(password));
  }

  async function start(attempt: () => ReturnType<typeof signIn>) {
    setBusy(true);
    setError(null);
    const result = await attempt();
    setBusy(false);
    if (!result.ok) {
      setError(result.message);
      return;
    }
    await navigate({ to: "/" });
  }

  return (
    <main className="flex min-h-screen items-center justify-center p-4">
      <Card className="w-full max-w-sm">
        <CardHeader>
          <CardTitle>Sign in to bia</CardTitle>
          <CardDescription>
            Enter the admin password. Lost it? Restart bia with <code>bia serve --reset-admin</code>
            .
          </CardDescription>
        </CardHeader>
        <CardContent>
          <form onSubmit={submit} className="flex flex-col gap-4">
            <div className="flex flex-col gap-2">
              <Label htmlFor="password">Password</Label>
              <Input
                id="password"
                type="password"
                autoComplete="current-password"
                autoFocus
                value={password}
                onChange={(event) => setPassword(event.target.value)}
              />
            </div>
            {error && (
              <p role="alert" className="text-sm text-destructive">
                {error}
              </p>
            )}
            <Button type="submit" disabled={busy || password === ""}>
              {busy ? "Signing in…" : "Sign in"}
            </Button>
          </form>
          <div className="mt-4 border-t pt-4">
            {supported ? (
              <Button
                type="button"
                variant="outline"
                className="w-full"
                disabled={busy}
                onClick={() => void start(signInWithPasskey)}
              >
                Sign in with a passkey
              </Button>
            ) : (
              <p role="note" className="text-sm text-muted-foreground">
                {PASSKEYS_NEED_HTTPS}
              </p>
            )}
          </div>
        </CardContent>
      </Card>
    </main>
  );
}
