import { createFileRoute, Link } from "@tanstack/react-router";
import { DefinitionEditor } from "@/editor/definition-editor";

export const Route = createFileRoute("/_app/workflows_/new")({
  component: NewWorkflow,
});

const STARTER = {
  schema_version: 1,
  id: "my-workflow",
  description: "What this Workflow does",
  triggers: [{ id: "by_hand", type: "manual" }],
  steps: [{ id: "pause", action: "flow.sleep", params: { duration: "1s" } }],
};

function NewWorkflow() {
  return (
    <section className="flex flex-col gap-4">
      <div>
        <Link to="/workflows" className="text-sm text-muted-foreground hover:underline">
          ← Workflows
        </Link>
        <h1 className="mt-1 text-2xl font-semibold">New Workflow</h1>
        <p className="text-sm text-muted-foreground">
          The Definition&apos;s <code>id</code> names the Workflow. Saving creates its first Draft.
        </p>
      </div>
      <DefinitionEditor workflowId={undefined} source={{ definition: STARTER }} />
    </section>
  );
}
