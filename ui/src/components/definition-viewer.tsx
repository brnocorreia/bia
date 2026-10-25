import { useMemo, useState } from "react";
import { stringify } from "yaml";
import { Button } from "@/components/ui/button";

type Format = "json" | "yaml";

/**
 * Renders a Definition read-only, as JSON or YAML (the form the DSL docs use). Stays mounted
 * while a Definition loads (`undefined`), so the chosen format survives switching Versions.
 */
export function DefinitionViewer({ definition }: { definition: unknown | undefined }) {
  const [format, setFormat] = useState<Format>("yaml");
  const text = useMemo(
    () =>
      definition === undefined
        ? "Loading…"
        : format === "json"
          ? JSON.stringify(definition, null, 2)
          : stringify(definition),
    [definition, format],
  );
  return (
    <div className="flex flex-col gap-2">
      <div role="group" aria-label="Format" className="flex gap-1">
        {(["yaml", "json"] as const).map((f) => (
          <Button
            key={f}
            size="sm"
            variant={format === f ? "default" : "outline"}
            aria-pressed={format === f}
            onClick={() => setFormat(f)}
          >
            {f.toUpperCase()}
          </Button>
        ))}
      </div>
      <pre
        aria-label="Definition"
        className="max-h-[32rem] overflow-auto rounded-md border bg-muted p-4 text-xs"
      >
        {text}
      </pre>
    </div>
  );
}
