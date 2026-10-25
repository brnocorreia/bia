import { createFileRoute, Link, useLocation } from "@tanstack/react-router";
import { useState } from "react";
import { messageOf } from "@/api/request";
import { useVersion, useVersions } from "@/api/workflows";
import { DryRunPanel } from "@/dry-run/dry-run-panel";
import { DefinitionEditor, type EditorSource } from "@/editor/definition-editor";

interface Search {
  /** The Version to start from; the latest when absent. */
  v?: number;
}

export const Route = createFileRoute("/_app/workflows_/$workflowId_/edit")({
  validateSearch: (search: Record<string, unknown>): Search => {
    const v = Number(search.v);
    return { v: Number.isInteger(v) && v > 0 ? v : undefined };
  },
  component: EditWorkflow,
});

function EditWorkflow() {
  const { workflowId } = Route.useParams();
  const { v } = Route.useSearch();
  const versions = useVersions(workflowId);
  const version = useVersion(workflowId, v ?? versions.data?.at(-1)?.number);
  const error = versions.error ?? version.error;
  const notice = useLocation({
    select: (location) => (location.state as { notice?: string }).notice,
  });

  // The editor keeps its own text after a save, which moves `v` to the new Draft: the Version
  // loaded first is the source, and later loads must not replace it.
  const [source, setSource] = useState<EditorSource>();
  if (!source && version.data) {
    setSource({
      definition: version.data.definition,
      version: { number: version.data.number, state: version.data.state },
    });
  }

  return (
    <section className="flex flex-col gap-4">
      <div>
        <Link
          to="/workflows/$workflowId"
          params={{ workflowId }}
          className="text-sm text-muted-foreground hover:underline"
        >
          ← {workflowId}
        </Link>
        <h1 className="mt-1 text-2xl font-semibold">Edit {workflowId}</h1>
      </div>
      {error && (
        <p role="alert" className="text-sm text-destructive">
          {messageOf(error)}
        </p>
      )}
      {!error && !source && <p className="text-sm text-muted-foreground">Loading…</p>}
      {source && (
        <DefinitionEditor
          workflowId={workflowId}
          source={source}
          initialNotice={notice}
          sidePanel={(context) => <DryRunPanel {...context} />}
        />
      )}
    </section>
  );
}
