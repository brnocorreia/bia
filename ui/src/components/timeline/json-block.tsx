import { useMemo, useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { hasSecretPlaceholder } from "./model";

/** Above this many characters a value is cut until the reader asks for all of it. */
export const PREVIEW_LIMIT = 4000;

/**
 * A JSON value behind a disclosure. It is closed by default and formats the value only once
 * opened, so a huge Output costs nothing until someone reads it, and even then only its first
 * `PREVIEW_LIMIT` characters are put in the page until "Show all" is pressed.
 */
export function JsonBlock({ label, value }: { label: string; value: unknown }) {
  const [open, setOpen] = useState(false);
  const [all, setAll] = useState(false);
  const placeholder = useMemo(() => hasSecretPlaceholder(value), [value]);
  const text = useMemo(
    () => (open ? (JSON.stringify(value, null, 2) ?? "undefined") : ""),
    [open, value],
  );
  const shown = all ? text : text.slice(0, PREVIEW_LIMIT);

  return (
    <details onToggle={(event) => setOpen(event.currentTarget.open)} className="text-xs">
      <summary className="cursor-pointer select-none py-0.5 text-muted-foreground">
        {label}
        {placeholder && (
          <Badge variant="outline" className="ml-2">
            secret placeholder
          </Badge>
        )}
      </summary>
      {open && (
        <div className="mt-1">
          <pre
            aria-label={label}
            className="max-h-96 overflow-auto rounded-md border bg-muted p-3 whitespace-pre-wrap break-all"
          >
            {shown}
          </pre>
          {!all && text.length > PREVIEW_LIMIT && (
            <Button size="sm" variant="outline" className="mt-1" onClick={() => setAll(true)}>
              Show all ({text.length.toLocaleString()} characters)
            </Button>
          )}
        </div>
      )}
    </details>
  );
}
