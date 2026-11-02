import { useRef, useState, type FormEvent } from "react";
import { useQuery } from "@tanstack/react-query";
import { changePassword, passwordChangeErrorMessage } from "@/api/user";
import { MIN_PASSWORD_LENGTH } from "@/api/setup";
import { currentTokenQuery } from "@/auth/scope";
import { signOut } from "@/auth/login";
import { useSessionState } from "@/auth/session";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { ConfirmDialog } from "./confirm-dialog";

/** The signed-in user's menu: change the password, sign out. */
export function UserMenu() {
  const [changing, setChanging] = useState(false);
  const menu = useRef<HTMLDetailsElement>(null);
  const token = useQuery({
    ...currentTokenQuery,
    enabled: useSessionState() === "signed-in",
  });

  return (
    <>
      <details ref={menu} className="relative">
        <summary className="cursor-pointer list-none rounded-md border px-3 py-1.5 text-center text-sm hover:bg-accent">
          Account{token.data ? ` (${token.data.scope})` : ""}
        </summary>
        <div className="absolute bottom-full mb-1 flex w-full flex-col gap-1 rounded-md border bg-card p-1 shadow-md">
          <Button
            variant="ghost"
            size="sm"
            onClick={() => {
              menu.current?.removeAttribute("open");
              setChanging(true);
            }}
          >
            Change password
          </Button>
          <Button variant="ghost" size="sm" onClick={() => void signOut()}>
            Sign out
          </Button>
        </div>
      </details>
      {/* Outside the `<details>`, whose closed content is not rendered. */}
      <ChangePasswordDialog open={changing} onClose={() => setChanging(false)} />
    </>
  );
}

function ChangePasswordDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [again, setAgain] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);

  function close() {
    setCurrent("");
    setNext("");
    setAgain("");
    setError(null);
    setDone(null);
    onClose();
  }

  async function submit(event?: FormEvent) {
    event?.preventDefault();
    if (done !== null) return close();
    if ([...next].length < MIN_PASSWORD_LENGTH) {
      return setError(`The new password needs at least ${MIN_PASSWORD_LENGTH} characters.`);
    }
    if (next !== again) return setError("The new passwords differ.");
    setBusy(true);
    setError(null);
    try {
      setDone((await changePassword(current, next)).revoked_sessions);
    } catch (failure) {
      setError(passwordChangeErrorMessage(failure));
    } finally {
      setBusy(false);
    }
  }

  return (
    <ConfirmDialog
      open={open}
      title="Change password"
      confirmLabel={done === null ? "Change password" : "Done"}
      busy={busy}
      error={error}
      onConfirm={() => void submit()}
      onCancel={close}
    >
      {done === null ? (
        <form onSubmit={submit} className="flex flex-col gap-3">
          <p>Every other Session is signed out. OAuth Grants are not affected.</p>
          <div className="flex flex-col gap-2">
            <Label htmlFor="current-password">Current password</Label>
            <Input
              id="current-password"
              type="password"
              autoComplete="current-password"
              value={current}
              onChange={(event) => setCurrent(event.target.value)}
            />
          </div>
          <div className="flex flex-col gap-2">
            <Label htmlFor="new-password">New password</Label>
            <Input
              id="new-password"
              type="password"
              autoComplete="new-password"
              value={next}
              onChange={(event) => setNext(event.target.value)}
            />
          </div>
          <div className="flex flex-col gap-2">
            <Label htmlFor="new-password-again">New password again</Label>
            <Input
              id="new-password-again"
              type="password"
              autoComplete="new-password"
              value={again}
              onChange={(event) => setAgain(event.target.value)}
            />
          </div>
        </form>
      ) : (
        <p>
          Password changed. {done} other {done === 1 ? "Session was" : "Sessions were"} signed out.
        </p>
      )}
    </ConfirmDialog>
  );
}
