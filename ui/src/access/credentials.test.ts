import { describe, expect, it } from "vitest";
import type { TokenInfo } from "@/api/tokens";
import { describeBrowser, groupByOrigin } from "./credentials";

function token(id: string, origin: TokenInfo["origin"]): TokenInfo {
  return {
    id,
    name: id,
    scope: "admin",
    origin,
    created_at: "2026-01-01T00:00:00Z",
    last_used_at: null,
    revoked_at: null,
    user_agent: null,
    grant_id: null,
  };
}

describe("groupByOrigin", () => {
  it("splits the list by origin and keeps the order", () => {
    const grouped = groupByOrigin([
      token("a", "manual"),
      token("b", "session"),
      token("c", "oauth"),
      token("d", "session"),
    ]);

    expect(grouped.sessions.map((t) => t.id)).toEqual(["b", "d"]);
    expect(grouped.oauth.map((t) => t.id)).toEqual(["c"]);
    expect(grouped.manual.map((t) => t.id)).toEqual(["a"]);
  });
});

describe("describeBrowser", () => {
  it("names the browser and system", () => {
    expect(
      describeBrowser("Mozilla/5.0 (X11; Linux x86_64; rv:130.0) Gecko/20100101 Firefox/130.0"),
    ).toBe("Firefox on Linux");
    expect(
      describeBrowser(
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 Chrome/130.0 Safari/537.36",
      ),
    ).toBe("Chrome on macOS");
  });

  it("says so when there is no User-Agent", () => {
    expect(describeBrowser(null)).toBe("Unknown browser");
  });
});
