import type { components } from "@/api/client";
import { Badge } from "@/components/ui/badge";

export type Diagnostic = components["schemas"]["Diagnostic"];

/** Diagnostics as the API returns them: severity, the JSON Pointer path, message, suggestion. */
export function DiagnosticsList({
  diagnostics,
  onSelect,
}: {
  diagnostics: Diagnostic[];
  /** Makes each entry a button, e.g. to jump to its place in an editor. */
  onSelect?: (diagnostic: Diagnostic) => void;
}) {
  if (diagnostics.length === 0) return null;
  return (
    <ul aria-label="Diagnostics" className="flex flex-col gap-2">
      {diagnostics.map((d, index) => (
        <li
          key={index}
          className={`rounded-md border p-3 text-sm text-foreground ${onSelect ? "cursor-pointer hover:bg-accent" : ""}`}
          onClick={onSelect && (() => onSelect(d))}
        >
          <div className="flex flex-wrap items-center gap-2">
            <Badge variant={d.severity === "error" ? "destructive" : "secondary"}>
              {d.severity}
            </Badge>
            <code className="text-xs">{d.path === "" ? "(document)" : d.path}</code>
            <span className="text-xs text-muted-foreground">{d.code}</span>
          </div>
          <p className="mt-1">{d.message}</p>
          {d.suggestion && <p className="mt-1 text-muted-foreground">Suggestion: {d.suggestion}</p>}
        </li>
      ))}
    </ul>
  );
}
