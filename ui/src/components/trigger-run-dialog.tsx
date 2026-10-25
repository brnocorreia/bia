import { useNavigate } from "@tanstack/react-router";
import { useState } from "react";
import { useTriggerRun } from "@/api/runs";
import { messageOf } from "@/api/request";
import { useWorkflows } from "@/api/workflows";
import { ConfirmDialog } from "@/components/confirm-dialog";

const SELECT_CLASS = "h-9 rounded-md border bg-background px-2 text-sm";

/** Starts a Run of a Workflow's Active Version from its manual Trigger, with JSON input. */
export function TriggerRunDialog({
  open,
  initialWorkflow,
  onClose,
}: {
  open: boolean;
  initialWorkflow?: string;
  onClose: () => void;
}) {
  const navigate = useNavigate();
  const workflows = useWorkflows();
  const trigger = useTriggerRun();
  const [chosen, setChosen] = useState<string | undefined>(undefined);
  const [text, setText] = useState("{}");
  const [parseError, setParseError] = useState<string | null>(null);

  const runnable = (workflows.data ?? []).filter((w) => w.active_version !== null);
  const workflowId = chosen ?? initialWorkflow ?? runnable[0]?.id;

  function submit() {
    let input: unknown;
    try {
      input = JSON.parse(text);
    } catch (error) {
      setParseError(`The input is not valid JSON: ${(error as Error).message}`);
      return;
    }
    setParseError(null);
    if (!workflowId) return;
    trigger.mutate(
      { workflowId, input },
      {
        onSuccess: (run) => {
          onClose();
          void navigate({ to: "/runs/$runId", params: { runId: run.id } });
        },
      },
    );
  }

  return (
    <ConfirmDialog
      open={open}
      title="Trigger a Run"
      confirmLabel="Trigger"
      busy={trigger.isPending}
      error={parseError ?? (trigger.error ? messageOf(trigger.error) : undefined)}
      onConfirm={submit}
      onCancel={() => {
        trigger.reset();
        setParseError(null);
        onClose();
      }}
    >
      {runnable.length === 0 ? (
        <p>No Workflow has an Active Version yet, so there is nothing to trigger.</p>
      ) : (
        <div className="flex flex-col gap-3">
          <label className="flex flex-col gap-1 text-foreground">
            Workflow
            <select
              className={SELECT_CLASS}
              value={workflowId}
              onChange={(event) => setChosen(event.target.value)}
            >
              {runnable.map((w) => (
                <option key={w.id} value={w.id}>
                  {w.id} (v{w.active_version})
                </option>
              ))}
            </select>
          </label>
          <label className="flex flex-col gap-1 text-foreground">
            Input (JSON)
            <textarea
              className="h-32 rounded-md border bg-background p-2 font-mono text-xs"
              value={text}
              spellCheck={false}
              onChange={(event) => setText(event.target.value)}
            />
          </label>
        </div>
      )}
    </ConfirmDialog>
  );
}
