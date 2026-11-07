import { createFileRoute } from "@tanstack/react-router";
import { useState, type FormEvent } from "react";
import { approveDevice, denyDevice, lookupDevice, type DeviceConsentRequest } from "@/api/grants";
import { ApiError, messageOf } from "@/api/request";
import { requireSession } from "@/auth/require-session";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { ConsentCard } from "@/oauth/consent-card";

interface Search {
  user_code?: string;
}

// Where `bia login --device` sends the person: enter the code the terminal shows, then see the
// same consent screen as any app. Outside the app shell, like the consent screen.
export const Route = createFileRoute("/device")({
  validateSearch: (search: Record<string, unknown>): Search => ({
    user_code:
      typeof search.user_code === "string" && search.user_code ? search.user_code : undefined,
  }),
  beforeLoad: ({ context, location }) => requireSession(context.queryClient, location),
  component: Device,
});

type Step =
  | { name: "enter" }
  | { name: "decide"; code: string; request: DeviceConsentRequest }
  | { name: "done"; approved: boolean };

function Device() {
  const { user_code: prefilled } = Route.useSearch();
  const [step, setStep] = useState<Step>({ name: "enter" });

  return (
    <main className="flex min-h-screen items-center justify-center p-4">
      <Card className="w-full max-w-lg">
        {step.name === "enter" && (
          <Enter
            initial={prefilled ?? ""}
            onFound={(code, request) => setStep({ name: "decide", code, request })}
          />
        )}
        {step.name === "decide" && (
          <>
            <CardContent className="pt-6 text-sm text-muted-foreground">
              Code <span className="font-mono text-foreground">{step.code}</span>. Check that it
              matches the one in your terminal.
            </CardContent>
            <ConsentCard
              client={step.request.client}
              requestedScope={step.request.requested_scope}
              resource={step.request.resource}
              onApprove={async (scope) => {
                await approveDevice(step.request.id, scope);
                setStep({ name: "done", approved: true });
              }}
              onDeny={async () => {
                await denyDevice(step.request.id);
                setStep({ name: "done", approved: false });
              }}
            />
          </>
        )}
        {step.name === "done" && (
          <>
            <CardHeader>
              <CardTitle>{step.approved ? "Approved" : "Denied"}</CardTitle>
              <CardDescription>
                {step.approved
                  ? "Return to your terminal: bia login finishes on its own."
                  : "Nothing was signed in. You can close this tab."}
              </CardDescription>
            </CardHeader>
          </>
        )}
      </Card>
    </main>
  );
}

function Enter({
  initial,
  onFound,
}: {
  initial: string;
  onFound: (code: string, request: DeviceConsentRequest) => void;
}) {
  const [code, setCode] = useState(initial);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function submit(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      onFound(code.trim().toUpperCase(), await lookupDevice(code));
    } catch (failure) {
      setError(
        failure instanceof ApiError && failure.code === "device_authorization_not_found"
          ? "No sign-in is waiting for that code. It may have run out: run bia login --device again."
          : messageOf(failure),
      );
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      <CardHeader>
        <CardTitle>Sign in a device</CardTitle>
        <CardDescription>
          Enter the code that <code>bia login --device</code> shows in your terminal.
        </CardDescription>
      </CardHeader>
      <CardContent>
        <form onSubmit={submit} className="flex flex-col gap-4">
          <div className="flex flex-col gap-2">
            <Label htmlFor="user-code">Code</Label>
            <Input
              id="user-code"
              autoComplete="off"
              autoCapitalize="characters"
              spellCheck={false}
              autoFocus
              className="font-mono uppercase"
              placeholder="XXXX-XXXX"
              value={code}
              onChange={(event) => setCode(event.target.value)}
            />
          </div>
          {error && (
            <p role="alert" className="text-sm text-destructive">
              {error}
            </p>
          )}
          <Button type="submit" disabled={busy || code.trim() === ""}>
            {busy ? "Checking…" : "Continue"}
          </Button>
        </form>
      </CardContent>
    </>
  );
}
