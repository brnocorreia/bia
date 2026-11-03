import { createFileRoute, Link, useNavigate } from "@tanstack/react-router";
import { useState, type FormEvent } from "react";
import { MIN_PASSWORD_LENGTH, setUp, setupErrorMessage } from "@/api/setup";
import { confirmSession } from "@/auth/login";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

interface Search {
  code?: string;
}

export const Route = createFileRoute("/setup")({
  validateSearch: (search: Record<string, unknown>): Search => ({
    code: typeof search.code === "string" && search.code ? search.code : undefined,
  }),
  component: Setup,
});

function Setup() {
  const { code: linkCode } = Route.useSearch();
  // Set when setup worked but the browser dropped the cookie it started the Session with.
  const [notSignedIn, setNotSignedIn] = useState<string | null>(null);

  return (
    <main className="flex min-h-screen items-center justify-center p-4">
      <Card className="w-full max-w-md">
        {notSignedIn ? (
          <NotSignedIn message={notSignedIn} />
        ) : (
          <SetupForm linkCode={linkCode} onNotSignedIn={setNotSignedIn} />
        )}
      </Card>
    </main>
  );
}

function SetupForm({
  linkCode,
  onNotSignedIn,
}: {
  linkCode: string | undefined;
  onNotSignedIn: (message: string) => void;
}) {
  const navigate = useNavigate();
  const [code, setCode] = useState(linkCode ?? "");
  const [password, setPassword] = useState("");
  const [again, setAgain] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const tooShort = [...password].length < MIN_PASSWORD_LENGTH;
  const differs = again !== "" && again !== password;

  async function submit(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await setUp(code.trim(), password);
      // Setup started the Session; setting up ends by signing in.
      const result = await confirmSession();
      if (result.ok) await navigate({ to: "/" });
      else onNotSignedIn(result.message);
    } catch (failure) {
      setError(setupErrorMessage(failure));
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      <CardHeader>
        <CardTitle>Set up bia</CardTitle>
        <CardDescription>
          Choose the admin password. It needs at least {MIN_PASSWORD_LENGTH} characters, and nothing
          else.
        </CardDescription>
      </CardHeader>
      <CardContent>
        <form onSubmit={submit} className="flex flex-col gap-4">
          {!linkCode && (
            <div className="flex flex-col gap-2">
              <Label htmlFor="code">Setup code</Label>
              <Input
                id="code"
                autoComplete="off"
                value={code}
                onChange={(event) => setCode(event.target.value)}
              />
              <p className="text-xs text-muted-foreground">
                The code in the Setup Link that <code>bia serve</code> logs.
              </p>
            </div>
          )}
          <div className="flex flex-col gap-2">
            <Label htmlFor="password">Password</Label>
            <Input
              id="password"
              type="password"
              autoComplete="new-password"
              value={password}
              onChange={(event) => setPassword(event.target.value)}
            />
          </div>
          <div className="flex flex-col gap-2">
            <Label htmlFor="password-again">Password again</Label>
            <Input
              id="password-again"
              type="password"
              autoComplete="new-password"
              value={again}
              onChange={(event) => setAgain(event.target.value)}
            />
            {differs && <p className="text-xs text-destructive">The passwords differ.</p>}
          </div>
          {error && (
            <p role="alert" className="text-sm text-destructive">
              {error}
            </p>
          )}
          <Button
            type="submit"
            disabled={busy || code.trim() === "" || tooShort || again !== password}
          >
            {busy ? "Setting up…" : "Set password"}
          </Button>
        </form>
      </CardContent>
    </>
  );
}

function NotSignedIn({ message }: { message: string }) {
  return (
    <>
      <CardHeader>
        <CardTitle>Setup complete</CardTitle>
        <CardDescription>Your password is set.</CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-4 text-sm">
        <p role="alert" className="text-destructive">
          {message}
        </p>
        <p>
          Once they are, <Link to="/login">sign in</Link> with the password you just set.
        </p>
      </CardContent>
    </>
  );
}
