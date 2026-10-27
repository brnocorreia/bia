import { createFileRoute, Link } from "@tanstack/react-router";
import { useState, type FormEvent } from "react";
import { MIN_PASSWORD_LENGTH, setUp, setupErrorMessage, type SetupDone } from "@/api/setup";
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
  // The Token lives in component state only: it is shown once and never cached (ADR-0057).
  const [done, setDone] = useState<SetupDone | null>(null);

  return (
    <main className="flex min-h-screen items-center justify-center p-4">
      <Card className="w-full max-w-md">
        {done ? <Complete done={done} /> : <SetupForm linkCode={linkCode} onDone={setDone} />}
      </Card>
    </main>
  );
}

function SetupForm({
  linkCode,
  onDone,
}: {
  linkCode: string | undefined;
  onDone: (done: SetupDone) => void;
}) {
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
      onDone(await setUp(code.trim(), password));
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

function Complete({ done }: { done: SetupDone }) {
  const { value } = done.token;
  return (
    <>
      <CardHeader>
        <CardTitle>Setup complete</CardTitle>
        <CardDescription>
          Your password is set. Until bia can sign you in with it, use this admin Token: copy it
          now, since it is shown only once.
        </CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-4 text-sm">
        <div className="flex items-center gap-2">
          <code
            data-testid="setup-token-value"
            className="break-all rounded bg-muted px-2 py-1 font-mono"
          >
            {value}
          </code>
          <Button
            variant="outline"
            size="sm"
            onClick={() => void navigator.clipboard?.writeText(value)}
          >
            Copy
          </Button>
        </div>
        <ol className="list-decimal space-y-1 pl-5">
          <li>
            Sign in to this UI by pasting the Token on the{" "}
            <Link to="/login" className="underline">
              sign-in screen
            </Link>
            .
          </li>
          <li>
            For the CLI, set <code>BIA_TOKEN</code> to it.
          </li>
        </ol>
      </CardContent>
    </>
  );
}
