import { useDiff, useVersion } from "@/api/workflows";
import { messageOf } from "@/api/request";
import { Badge } from "@/components/ui/badge";
import { pointerGet } from "@/lib/json-pointer";

interface PatchOperation {
  op: string;
  path: string;
  from?: string;
  value?: unknown;
}

const show = (value: unknown) => (value === undefined ? "—" : JSON.stringify(value));

/**
 * The structural difference between two Versions: the API's RFC 6902 patch, one row per
 * changed field, with the old value read from the `from` Definition.
 */
export function VersionDiff({
  workflowId,
  from,
  to,
}: {
  workflowId: string;
  from: number;
  to: number;
}) {
  const diff = useDiff(workflowId, from, to);
  const before = useVersion(workflowId, from);

  if (diff.error) {
    return (
      <p role="alert" className="text-sm text-destructive">
        {messageOf(diff.error)}
      </p>
    );
  }
  if (!diff.data || !before.data) return <p className="text-sm text-muted-foreground">Loading…</p>;

  const patch = diff.data.patch as unknown as PatchOperation[];
  if (patch.length === 0) {
    return (
      <p className="text-sm text-muted-foreground">
        Versions {from} and {to} have identical Definitions.
      </p>
    );
  }
  return (
    <table
      aria-label={`Changes from version ${from} to version ${to}`}
      className="w-full text-left text-sm"
    >
      <thead className="border-b text-muted-foreground">
        <tr>
          <th className="py-2 pr-4 font-medium">Change</th>
          <th className="py-2 pr-4 font-medium">Path</th>
          <th className="py-2 pr-4 font-medium">v{from}</th>
          <th className="py-2 font-medium">v{to}</th>
        </tr>
      </thead>
      <tbody>
        {patch.map((operation, index) => (
          <tr key={index} className="border-b align-top">
            <td className="py-2 pr-4">
              <Badge variant={operation.op === "remove" ? "destructive" : "secondary"}>
                {operation.op}
              </Badge>
            </td>
            <td className="py-2 pr-4">
              <code className="text-xs">{operation.path}</code>
            </td>
            <td className="py-2 pr-4">
              <code className="text-xs break-all">
                {operation.op === "add"
                  ? "—"
                  : show(pointerGet(before.data.definition, operation.path))}
              </code>
            </td>
            <td className="py-2">
              <code className="text-xs break-all">
                {operation.op === "remove"
                  ? "—"
                  : operation.from !== undefined
                    ? `from ${operation.from}`
                    : show(operation.value)}
              </code>
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}
