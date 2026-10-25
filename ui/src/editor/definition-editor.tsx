import { useBlocker } from "@tanstack/react-router";
import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ReactNode } from "react";
import { useNavigate } from "@tanstack/react-router";
import { messageOf } from "@/api/request";
import { useDefinitionSchema, useSaveDraft, validateDefinition } from "@/api/editor";
import { ConfirmDialog } from "@/components/confirm-dialog";
import { DiagnosticsList, type Diagnostic } from "@/components/diagnostics-list";
import { Button } from "@/components/ui/button";
import { formatText, parseText, yamlHasComments } from "./convert";
import type { EditorHandle } from "./monaco-editor";
import type { Format } from "./path-range";

// Monaco is large; the editor screen is the only one that needs it.
const MonacoEditor = lazy(() => import("./monaco-editor"));

const VALIDATE_DEBOUNCE_MS = 500;

/**
 * What the editor shows a side panel (BIA-33's Dry Run, for one): the Definition as parsed
 * from the current text, or `undefined` while the text does not parse.
 */
export interface EditorContext {
  /** The Workflow being edited; `undefined` for one that has no Draft yet. */
  workflowId: string | undefined;
  /** The Definition in the editor right now, parsed. `undefined` while the text has a syntax error. */
  definition: unknown | undefined;
  /** Whether the current text differs from what was last loaded or saved. */
  dirty: boolean;
  /** The latest server Diagnostics for `definition`; `undefined` until the first validation. */
  diagnostics: Diagnostic[] | undefined;
}

/** What the editor starts from. */
export interface EditorSource {
  /** The Definition to load. */
  definition: unknown;
  /** The Version it came from, if any. */
  version?: { number: number; state: "draft" | "published" };
}

type Validation =
  | { status: "idle" }
  | { status: "running" }
  | { status: "done"; diagnostics: Diagnostic[]; valid: boolean }
  | { status: "failed"; message: string };

/**
 * The Definition editor: Monaco in JSON or YAML, completion from the DSL schema, the server's
 * Diagnostics underlined at their path, and Save as Draft. `sidePanel` renders beside the
 * editor with the current Definition, so a feature that works on it (Dry Run) adds no logic
 * here. Mount it with a `key` per source, so loading another Version starts a fresh session.
 */
export function DefinitionEditor({
  workflowId,
  source,
  sidePanel,
  initialNotice,
}: {
  workflowId: string | undefined;
  source: EditorSource;
  sidePanel?: (context: EditorContext) => ReactNode;
  /** A message to show on mount, such as the save that led here from the new-Workflow screen. */
  initialNotice?: string;
}) {
  const navigate = useNavigate();
  const schema = useDefinitionSchema();
  const save = useSaveDraft(workflowId);

  const [format, setFormat] = useState<Format>("yaml");
  const [text, setText] = useState(() => formatText(source.definition, "yaml"));
  // What is stored on the server, as the canonical JSON the dirty check compares against.
  const [saved, setSaved] = useState(() => JSON.stringify(source.definition));
  const [validation, setValidation] = useState<Validation>({ status: "idle" });
  const [version, setVersion] = useState(source.version);
  const [notice, setNotice] = useState<string | null>(initialNotice ?? null);
  const [switchError, setSwitchError] = useState<string | null>(null);
  const handle = useRef<EditorHandle | null>(null);
  const onHandle = useCallback((next: EditorHandle) => {
    handle.current = next;
  }, []);

  const parsed = useMemo(() => parseText(text, format), [text, format]);
  const definition = parsed.ok ? parsed.value : undefined;
  const dirty = parsed.ok ? JSON.stringify(parsed.value) !== saved : true;
  const dirtyRef = useRef(dirty);
  useEffect(() => {
    dirtyRef.current = dirty;
  }, [dirty]);

  // Validate after the user stops typing. A sequence number drops answers to stale text.
  const sequence = useRef(0);
  useEffect(() => {
    if (definition === undefined) {
      sequence.current++;
      return;
    }
    const mine = ++sequence.current;
    const timer = setTimeout(() => {
      setValidation({ status: "running" });
      validateDefinition(definition).then(
        (report) => {
          if (mine === sequence.current) {
            setValidation({ status: "done", diagnostics: report.diagnostics, valid: report.valid });
          }
        },
        (error: unknown) => {
          if (mine === sequence.current) {
            setValidation({ status: "failed", message: messageOf(error) });
          }
        },
      );
    }, VALIDATE_DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [definition]);

  const blocker = useBlocker({
    shouldBlockFn: () => dirtyRef.current,
    enableBeforeUnload: () => dirtyRef.current,
    withResolver: true,
  });

  function switchFormat(next: Format) {
    if (next === format) return;
    if (!parsed.ok) {
      setSwitchError("Fix the syntax error before switching format.");
      return;
    }
    setSwitchError(null);
    setText(formatText(parsed.value, next));
    setFormat(next);
  }

  function saveDraft(replace?: number) {
    if (!parsed.ok) return;
    setNotice(null);
    save.mutate(
      { definition: parsed.value, replace },
      {
        onSuccess: (result) => {
          setSaved(JSON.stringify(parsed.value));
          dirtyRef.current = false;
          setValidation({
            status: "done",
            diagnostics: result.diagnostics,
            valid: result.diagnostics.every((d) => d.severity !== "error"),
          });
          const number = result.version.number;
          setVersion({ number, state: "draft" });
          const message =
            replace === undefined ? `Saved as Draft v${number}.` : `Replaced Draft v${number}.`;
          setNotice(message);
          const id = result.version.workflow_id;
          void navigate({
            to: "/workflows/$workflowId/edit",
            params: { workflowId: id },
            search: { v: number },
            replace: true,
            // Carries the message across the new-Workflow screen, which this unmounts.
            state: { notice: message } as never,
          });
        },
      },
    );
  }

  const diagnostics = validation.status === "done" ? validation.diagnostics : undefined;
  const draft = version?.state === "draft" ? version.number : undefined;
  const context: EditorContext = {
    workflowId,
    definition,
    dirty,
    diagnostics,
  };
  const errorCount = diagnostics?.filter((d) => d.severity === "error").length ?? 0;
  const hasComments = format === "yaml" && yamlHasComments(text);

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center gap-2">
        <div role="group" aria-label="Format" className="flex gap-1">
          {(["yaml", "json"] as const).map((f) => (
            <Button
              key={f}
              size="sm"
              variant={format === f ? "default" : "outline"}
              aria-pressed={format === f}
              onClick={() => switchFormat(f)}
            >
              {f.toUpperCase()}
            </Button>
          ))}
        </div>
        <div className="ml-auto flex flex-wrap items-center gap-2">
          {draft !== undefined && (
            <Button
              variant="outline"
              disabled={!parsed.ok || save.isPending}
              onClick={() => saveDraft(draft)}
            >
              Replace Draft v{draft}
            </Button>
          )}
          <Button disabled={!parsed.ok || save.isPending} onClick={() => saveDraft()}>
            Save as new Draft
          </Button>
        </div>
      </div>

      {switchError && (
        <p role="alert" className="text-sm text-destructive">
          {switchError}
        </p>
      )}
      {save.error && (
        <p role="alert" className="text-sm text-destructive">
          {messageOf(save.error)}
        </p>
      )}
      {notice && (
        <p role="status" className="text-sm">
          {notice}
        </p>
      )}
      {hasComments && (
        <p className="text-sm text-muted-foreground">
          YAML comments are not kept: the server stores the Definition as JSON.
        </p>
      )}

      <div className="grid gap-4 lg:grid-cols-[minmax(0,1fr)_minmax(0,24rem)]">
        <div className="flex min-w-0 flex-col gap-3">
          <div className="h-[28rem] overflow-hidden rounded-md border">
            <Suspense
              fallback={<p className="p-4 text-sm text-muted-foreground">Loading editor…</p>}
            >
              <MonacoEditor
                value={text}
                format={format}
                schema={schema.data}
                diagnostics={diagnostics ?? []}
                syntaxError={parsed.ok ? null : parsed.error}
                onChange={setText}
                onHandle={onHandle}
              />
            </Suspense>
          </div>
          <p role="status" aria-label="Validation" className="text-sm text-muted-foreground">
            {!parsed.ok
              ? `Syntax error: ${parsed.error.message}`
              : validation.status === "running"
                ? "Validating…"
                : validation.status === "failed"
                  ? `Validation failed: ${validation.message}`
                  : validation.status === "done"
                    ? errorCount === 0
                      ? "No errors."
                      : `${errorCount} ${errorCount === 1 ? "error" : "errors"}.`
                    : ""}
            {dirty && " Unsaved changes."}
          </p>
          {diagnostics && diagnostics.length > 0 && (
            <div>
              <h2 className="mb-2 text-lg font-semibold">Diagnostics</h2>
              <DiagnosticsList
                diagnostics={diagnostics}
                onSelect={(d) => handle.current?.reveal(d.path)}
              />
            </div>
          )}
        </div>
        {sidePanel && (
          <aside aria-label="Side panel" className="min-w-0">
            {sidePanel(context)}
          </aside>
        )}
      </div>

      <ConfirmDialog
        open={blocker.status === "blocked"}
        title="Discard unsaved changes?"
        confirmLabel="Discard changes"
        onConfirm={() => blocker.proceed?.()}
        onCancel={() => blocker.reset?.()}
      >
        The Definition in the editor has changes that are not saved as a Draft.
      </ConfirmDialog>
    </div>
  );
}
