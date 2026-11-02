import { useEffect, useRef, useState, type FormEvent } from "react";
import { useQueryClient } from "@tanstack/react-query";
import {
  passkeyErrorMessage,
  passkeyKeys,
  registerPasskey,
  removePasskey,
  renamePasskey,
  usePasskeys,
  type PasskeyInfo,
} from "@/api/passkeys";
import { PASSKEYS_NEED_HTTPS, passkeysSupported } from "@/auth/webauthn";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { formatTime } from "@/lib/format";

/** The passkeys the admin signs in with: list, add, rename, remove. */
export function PasskeysDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  const ref = useRef<HTMLDialogElement>(null);
  const supported = passkeysSupported();

  useEffect(() => {
    const dialog = ref.current;
    if (!dialog) return;
    if (open && !dialog.open) dialog.showModal();
    if (!open && dialog.open) dialog.close();
  }, [open]);

  return (
    <dialog
      ref={ref}
      aria-labelledby="passkeys-title"
      onCancel={(event) => {
        event.preventDefault();
        onClose();
      }}
      className="m-auto w-full max-w-lg rounded-lg border bg-card p-6 text-card-foreground shadow-lg backdrop:bg-black/40"
    >
      <h2 id="passkeys-title" className="text-lg font-semibold">
        Passkeys
      </h2>
      <div className="mt-2 text-sm text-muted-foreground">
        {supported ? open && <PasskeyList /> : <p role="note">{PASSKEYS_NEED_HTTPS}</p>}
      </div>
      <div className="mt-6 flex justify-end">
        <Button variant="outline" onClick={onClose}>
          Close
        </Button>
      </div>
    </dialog>
  );
}

function PasskeyList() {
  const queryClient = useQueryClient();
  const passkeys = usePasskeys(true);
  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function act(action: () => Promise<unknown>) {
    setBusy(true);
    setError(null);
    try {
      await action();
      await queryClient.invalidateQueries({ queryKey: passkeyKeys.all });
      return true;
    } catch (failure) {
      setError(passkeyErrorMessage(failure));
      return false;
    } finally {
      setBusy(false);
    }
  }

  async function add(event: FormEvent) {
    event.preventDefault();
    if (await act(() => registerPasskey(name.trim()))) setName("");
  }

  return (
    <div className="flex flex-col gap-4">
      <p>A passkey signs you in instead of the password. The password stays, for recovery.</p>
      {passkeys.isPending && <p>Loading…</p>}
      {passkeys.error && <p role="alert">{passkeyErrorMessage(passkeys.error)}</p>}
      {passkeys.data?.length === 0 && <p>No passkeys yet.</p>}
      <ul className="flex flex-col gap-2">
        {passkeys.data?.map((passkey) => (
          <PasskeyRow key={passkey.id} passkey={passkey} busy={busy} act={act} />
        ))}
      </ul>
      <form onSubmit={add} className="flex items-end gap-2">
        <div className="flex flex-1 flex-col gap-2">
          <Label htmlFor="passkey-name">Name for a new passkey</Label>
          <Input
            id="passkey-name"
            placeholder="laptop"
            value={name}
            onChange={(event) => setName(event.target.value)}
          />
        </div>
        <Button type="submit" disabled={busy || name.trim() === ""}>
          Add passkey
        </Button>
      </form>
      {error && (
        <p role="alert" className="text-destructive">
          {error}
        </p>
      )}
    </div>
  );
}

function PasskeyRow({
  passkey,
  busy,
  act,
}: {
  passkey: PasskeyInfo;
  busy: boolean;
  act: (action: () => Promise<unknown>) => Promise<boolean>;
}) {
  const [renaming, setRenaming] = useState<string | null>(null);
  const [confirmingRemoval, setConfirmingRemoval] = useState(false);

  return (
    <li className="flex items-center justify-between gap-2 rounded-md border p-2">
      {renaming === null ? (
        <div className="min-w-0">
          <div className="truncate font-medium text-foreground">{passkey.name}</div>
          <div className="text-xs">
            Added {formatTime(passkey.created_at)}
            {passkey.last_used_at
              ? `, last used ${formatTime(passkey.last_used_at)}`
              : ", never used"}
          </div>
        </div>
      ) : (
        <form
          className="flex flex-1 gap-2"
          onSubmit={async (event) => {
            event.preventDefault();
            if (await act(() => renamePasskey(passkey.id, renaming.trim()))) setRenaming(null);
          }}
        >
          <Input
            aria-label={`New name for ${passkey.name}`}
            value={renaming}
            onChange={(event) => setRenaming(event.target.value)}
          />
          <Button type="submit" size="sm" disabled={busy || renaming.trim() === ""}>
            Save
          </Button>
          <Button type="button" variant="ghost" size="sm" onClick={() => setRenaming(null)}>
            Cancel
          </Button>
        </form>
      )}
      {renaming === null && (
        <div className="flex shrink-0 gap-1">
          <Button
            variant="ghost"
            size="sm"
            disabled={busy}
            aria-label={`Rename ${passkey.name}`}
            onClick={() => setRenaming(passkey.name)}
          >
            Rename
          </Button>
          {confirmingRemoval ? (
            <>
              <Button
                variant="destructive"
                size="sm"
                disabled={busy}
                aria-label={`Confirm removing ${passkey.name}`}
                onClick={async () => {
                  await act(() => removePasskey(passkey.id));
                  setConfirmingRemoval(false);
                }}
              >
                Remove for good
              </Button>
              <Button variant="ghost" size="sm" onClick={() => setConfirmingRemoval(false)}>
                Keep
              </Button>
            </>
          ) : (
            <Button
              variant="ghost"
              size="sm"
              disabled={busy}
              aria-label={`Remove ${passkey.name}`}
              onClick={() => setConfirmingRemoval(true)}
            >
              Remove
            </Button>
          )}
        </div>
      )}
    </li>
  );
}
