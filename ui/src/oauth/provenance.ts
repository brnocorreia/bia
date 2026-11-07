import type { ConsentRequest, GrantInfo } from "@/api/grants";

type Client = Pick<ConsentRequest["client"], "provenance" | "host">;

/** Where a client's identity comes from, in words for the person approving it. */
export function describeProvenance({ provenance, host }: Client): string {
  switch (provenance) {
    case "cimd":
      return host ? `from ${host}` : "from its own website";
    case "builtin":
      return "built into bia";
    case "dynamic":
      return "self-registered, unverified";
  }
}

/** What a Grant's tokens are good for. */
export function describeResource(resource: string): string {
  return resource.endsWith("/mcp") ? "the MCP endpoint only" : "the whole API and the MCP endpoint";
}

export function describeGrantClient(grant: Pick<GrantInfo, "provenance" | "client_host">): string {
  return describeProvenance({ provenance: grant.provenance, host: grant.client_host });
}

/** Whether a Scope is the broader of the two a person can grant. */
export function isBroader(scope: "agent" | "admin"): boolean {
  return scope === "admin";
}
