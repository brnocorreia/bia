import { createFileRoute, redirect, useNavigate } from "@tanstack/react-router";
import { useState, type FormEvent } from "react";
import { verifyToken } from "@/auth/login";
import { session } from "@/auth/session";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { getConfig } from "@/config";

export const Route = createFileRoute("/login")({
  beforeLoad: () => {
    if (session.getToken()) throw redirect({ to: "/" });
  },
  component: Login,
});

function Login() {
  const navigate = useNavigate();
  const [token, setToken] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function submit(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError(null);
    const value = token.trim();
    const result = await verifyToken(getConfig().apiUrl, value);
    setBusy(false);
    if (!result.ok) {
      setError(result.message);
      return;
    }
    session.setToken(value);
    await navigate({ to: "/" });
  }

  return (
    <main className="flex min-h-screen items-center justify-center p-4">
      <Card className="w-full max-w-sm">
        <CardHeader>
          <CardTitle>Sign in to bia</CardTitle>
          <CardDescription>
            Paste an API Token. Create one with <code>bia token create</code>.
          </CardDescription>
        </CardHeader>
        <CardContent>
          <form onSubmit={submit} className="flex flex-col gap-4">
            <div className="flex flex-col gap-2">
              <Label htmlFor="token">Token</Label>
              <Input
                id="token"
                type="password"
                autoComplete="off"
                placeholder="bia_…"
                value={token}
                onChange={(event) => setToken(event.target.value)}
              />
            </div>
            {error && (
              <p role="alert" className="text-sm text-destructive">
                {error}
              </p>
            )}
            <Button type="submit" disabled={busy || token.trim() === ""}>
              {busy ? "Checking…" : "Sign in"}
            </Button>
          </form>
        </CardContent>
      </Card>
    </main>
  );
}
