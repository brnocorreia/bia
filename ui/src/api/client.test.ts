import { afterEach, describe, expect, it, vi } from "vitest";
import { Session } from "@/auth/session";
import { createApi } from "./client";

function apiReplying(status: number, body: unknown, session: Session) {
  const seen: Request[] = [];
  vi.stubGlobal("fetch", (request: Request) => {
    seen.push(request);
    return Promise.resolve(
      new Response(JSON.stringify(body), {
        status,
        headers: { "content-type": "application/json" },
      }),
    );
  });
  return { api: createApi("http://bia.test", session), seen };
}

afterEach(() => vi.unstubAllGlobals());

describe("the API client", () => {
  it("sends the Session cookie with every request and no Authorization header", async () => {
    const { api, seen } = apiReplying(200, {}, new Session());
    await api.GET("/v1/workflows");
    expect(seen[0]?.credentials).toBe("include");
    expect(seen[0]?.headers.has("Authorization")).toBe(false);
    expect(seen[0]?.url).toBe("http://bia.test/v1/workflows");
  });

  it.each(["missing_token", "invalid_token"])(
    "ends the session when the API answers 401 %s",
    async (code) => {
      const session = new Session();
      session.signedIn();
      const { api } = apiReplying(401, { code, message: "" }, session);
      await api.GET("/v1/workflows");
      expect(session.getState()).toBe("signed-out");
    },
  );

  it("keeps the session on a 401 that is only a wrong password", async () => {
    const session = new Session();
    session.signedIn();
    const { api } = apiReplying(401, { code: "invalid_credentials", message: "" }, session);
    await api.GET("/v1/workflows");
    expect(session.getState()).toBe("signed-in");
  });

  it("keeps the session on other failures", async () => {
    const session = new Session();
    session.signedIn();
    const { api } = apiReplying(403, { code: "insufficient_scope", message: "" }, session);
    await api.GET("/v1/workflows");
    expect(session.getState()).toBe("signed-in");
  });
});

describe("the session", () => {
  it("starts unknown and tells subscribers only about changes", () => {
    const session = new Session();
    const listener = vi.fn();
    session.subscribe(listener);
    expect(session.getState()).toBe("unknown");

    session.signedIn();
    session.signedIn();
    session.signedOut();

    expect(listener).toHaveBeenCalledTimes(2);
    expect(session.getState()).toBe("signed-out");
  });

  it("is not kept in any browser storage", () => {
    const setItem = vi.spyOn(Storage.prototype, "setItem");
    new Session().signedIn();
    expect(setItem).not.toHaveBeenCalled();
  });
});
