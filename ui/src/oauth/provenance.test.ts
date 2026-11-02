import { describe, expect, it } from "vitest";
import { describeProvenance, describeResource } from "./provenance";

describe("describeProvenance", () => {
  it("names the host a metadata document came from", () => {
    expect(describeProvenance({ provenance: "cimd", host: "claude.ai" })).toBe("from claude.ai");
  });

  it("calls a client that registered itself unverified", () => {
    expect(describeProvenance({ provenance: "dynamic", host: null })).toBe(
      "self-registered, unverified",
    );
  });

  it("says a built-in client is built in", () => {
    expect(describeProvenance({ provenance: "builtin", host: null })).toBe("built into bia");
  });
});

describe("describeResource", () => {
  it("tells an MCP-only audience from the whole API", () => {
    expect(describeResource("https://bia.example.com/mcp")).toBe("the MCP endpoint only");
    expect(describeResource("https://bia.example.com")).toBe("the whole API and the MCP endpoint");
  });
});
