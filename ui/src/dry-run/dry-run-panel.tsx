import { useEffect, useMemo, useState } from "react";
import { useActions, useDryRun } from "@/api/dry-run";
import { ApiError, messageOf } from "@/api/request";
import { DiagnosticsList } from "@/components/diagnostics-list";
import { StatusBadge } from "@/components/status-badge";
import { Timeline } from "@/components/timeline/timeline";
import { Button } from "@/components/ui/button";
import type { EditorContext } from "@/editor/definition-editor";
import {
  buildRequest,
  collectMockableSteps,
  loadDraft,
  mockTemplates,
  pretty,
  saveDraft,
  type DryRunDraft,
} from "./model";

const FIELD = "w-full rounded-md border bg-background p-2 font-mono text-xs";

/**
 * Dry-runs the Definition as it is in the editor, saved or not. Mocks are JSON in the API's own
 * format (ADR-0046), so a Mock written here can be pasted into a CLI file as it is.
 */
export function DryRunPanel({ workflowId, definition, dirty }: EditorContext) {
  const actions = useActions();
  const dryRun = useDryRun();
  const [draft, setDraft] = useState<DryRunDraft>(() => loadDraft(workflowId));
  const [formErrors, setFormErrors] = useState<string[]>([]);

  useEffect(() => saveDraft(workflowId, draft), [workflowId, draft]);

  const steps = useMemo(() => collectMockableSteps(definition), [definition]);
  const mocked = Object.keys(draft.mocks);
  const unmocked = steps.filter((step) => !mocked.includes(step.id));
  const exampleOutput = (action: string) =>
    actions.data?.find((a) => a.name === action)?.examples[0]?.output ?? {};

  const actionOf = (id: string) => steps.find((step) => step.id === id)?.action ?? "";

  function setMock(id: string, text: string) {
    setDraft((d) => ({ ...d, mocks: { ...d.mocks, [id]: text } }));
  }

  function run() {
    const built = buildRequest(draft);
    if (!built.ok) {
      setFormErrors(built.errors);
      return;
    }
    setFormErrors([]);
    dryRun.mutate({ definition, input: built.input, mocks: built.mocks });
  }

  const error = dryRun.error;
  const diagnostics = error instanceof ApiError ? error.body?.diagnostics : undefined;

  return (
    <section aria-label="Dry Run" className="flex flex-col gap-4 rounded-md border p-3">
      <div>
        <h2 className="text-lg font-semibold">Dry Run</h2>
        <p className="text-sm text-muted-foreground">
          Runs {dirty ? "the unsaved Definition in the editor" : "the Definition in the editor"}{" "}
          without storing anything. Pure Actions run for real; others return their Mock, or their
          first example.
        </p>
      </div>

      <label className="flex flex-col gap-1 text-sm font-medium">
        Input (JSON)
        <textarea
          className={FIELD}
          rows={4}
          value={draft.input}
          onChange={(e) => setDraft((d) => ({ ...d, input: e.target.value }))}
        />
      </label>

      <div className="flex flex-col gap-2">
        <h3 className="text-sm font-medium">Mocks</h3>
        {mocked.length === 0 && (
          <p className="text-sm text-muted-foreground">
            No Mocks: Steps use their Action&apos;s first example.
          </p>
        )}
        {mocked.map((id) => (
          <div key={id} className="flex flex-col gap-1 rounded-md border p-2">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <code className="text-sm">{id}</code>
              <div className="flex flex-wrap gap-1">
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() => setMock(id, pretty(mockTemplates.error()))}
                >
                  Fail
                </Button>
                <Button
                  variant="outline"
                  size="sm"
                  onClick={() =>
                    setMock(id, pretty(mockTemplates.attempts(exampleOutput(actionOf(id)))))
                  }
                >
                  Fail once, then succeed
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  onClick={() =>
                    setDraft((d) => {
                      const rest = { ...d.mocks };
                      delete rest[id];
                      return { ...d, mocks: rest };
                    })
                  }
                >
                  Remove
                </Button>
              </div>
            </div>
            <textarea
              aria-label={`Mock for ${id}`}
              className={FIELD}
              rows={5}
              value={draft.mocks[id]}
              onChange={(e) => setMock(id, e.target.value)}
            />
          </div>
        ))}
        {unmocked.length > 0 ? (
          <MockPicker
            steps={unmocked}
            onAdd={(step) =>
              setMock(step.id, pretty(mockTemplates.output(exampleOutput(step.action))))
            }
          />
        ) : (
          steps.length === 0 && (
            <p className="text-sm text-muted-foreground">
              The Definition has no Step that can be Mocked.
            </p>
          )
        )}
        {mocked.length > 0 && (
          <div className="flex flex-wrap gap-2 text-xs text-muted-foreground">
            Mock shapes:
            <code>{`{"output": …}`}</code>
            <code>{`{"error": {"class", "message"}}`}</code>
            <code>{`{"attempts": [ … ]}`}</code>
          </div>
        )}
        {mocked.length > 0 && (
          <div className="flex flex-wrap gap-2">
            <Button
              variant="outline"
              size="sm"
              onClick={() => navigator.clipboard?.writeText(pretty(buildMocksOrEmpty(draft)))}
            >
              Copy Mocks as JSON
            </Button>
          </div>
        )}
      </div>

      <Button onClick={run} disabled={definition === undefined || dryRun.isPending}>
        {dryRun.isPending ? "Running…" : "Run Dry Run"}
      </Button>
      {definition === undefined && (
        <p className="text-sm text-muted-foreground">Fix the syntax error to run a Dry Run.</p>
      )}

      {formErrors.length > 0 && (
        <ul role="alert" className="text-sm text-destructive">
          {formErrors.map((message) => (
            <li key={message}>{message}</li>
          ))}
        </ul>
      )}

      {error && (
        <div className="flex flex-col gap-2">
          <p role="alert" className="text-sm text-destructive">
            {error instanceof ApiError && error.code === "invalid_definition"
              ? "The Definition does not validate, so nothing ran:"
              : messageOf(error)}
          </p>
          {diagnostics && <DiagnosticsList diagnostics={diagnostics} />}
        </div>
      )}

      {dryRun.data && !error && (
        <div className="flex flex-col gap-3">
          <div className="flex items-center gap-2 text-sm">
            <span className="font-medium">Dry Run result</span>
            <StatusBadge status={dryRun.data.status} />
          </div>
          {dryRun.data.diagnostics.length > 0 && (
            <DiagnosticsList diagnostics={dryRun.data.diagnostics} />
          )}
          <Timeline entries={dryRun.data.timeline} dryRun />
        </div>
      )}
    </section>
  );
}

function buildMocksOrEmpty(draft: DryRunDraft) {
  const built = buildRequest(draft);
  return built.ok ? built.mocks : {};
}

function MockPicker({
  steps,
  onAdd,
}: {
  steps: { id: string; action: string }[];
  onAdd: (step: { id: string; action: string }) => void;
}) {
  const [chosen, setChosen] = useState<string>("");
  const step = steps.find((s) => s.id === chosen) ?? steps[0];
  return (
    <div className="flex flex-wrap items-center gap-2">
      <select
        aria-label="Step to Mock"
        className="h-9 rounded-md border bg-background px-2 text-sm"
        value={step?.id}
        onChange={(e) => setChosen(e.target.value)}
      >
        {steps.map((s) => (
          <option key={s.id} value={s.id}>
            {s.id} ({s.action})
          </option>
        ))}
      </select>
      <Button variant="outline" size="sm" onClick={() => step && onAdd(step)}>
        Add Mock
      </Button>
    </div>
  );
}
