import { useEffect, useRef, type ReactNode } from "react";
import { Button } from "@/components/ui/button";

/**
 * A modal that asks before a consequential action. The native `<dialog>` gives focus
 * trapping, Escape and the backdrop, so no dependency is needed. `error` keeps a failed
 * action visible inside the dialog, where the user decided to do it.
 */
export function ConfirmDialog({
  open,
  title,
  children,
  confirmLabel,
  busy = false,
  error,
  onConfirm,
  onCancel,
}: {
  open: boolean;
  title: string;
  children?: ReactNode;
  confirmLabel: string;
  busy?: boolean;
  error?: ReactNode;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const ref = useRef<HTMLDialogElement>(null);

  useEffect(() => {
    const dialog = ref.current;
    if (!dialog) return;
    if (open && !dialog.open) dialog.showModal();
    if (!open && dialog.open) dialog.close();
  }, [open]);

  return (
    <dialog
      ref={ref}
      aria-labelledby="confirm-title"
      onCancel={(event) => {
        event.preventDefault();
        onCancel();
      }}
      className="m-auto w-full max-w-lg rounded-lg border bg-card p-6 text-card-foreground shadow-lg backdrop:bg-black/40"
    >
      <h2 id="confirm-title" className="text-lg font-semibold">
        {title}
      </h2>
      <div className="mt-2 text-sm text-muted-foreground">{children}</div>
      {error && (
        <div role="alert" className="mt-3 text-sm text-destructive">
          {error}
        </div>
      )}
      <div className="mt-6 flex justify-end gap-2">
        <Button variant="outline" onClick={onCancel} disabled={busy}>
          Cancel
        </Button>
        <Button onClick={onConfirm} disabled={busy}>
          {busy ? "Working…" : confirmLabel}
        </Button>
      </div>
    </dialog>
  );
}
