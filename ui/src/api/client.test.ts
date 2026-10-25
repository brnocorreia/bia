import { describe, expect, it, vi } from "vitest";
import { Session } from "@/auth/session";
import { createApi } from "./client";

function memoryStorage() {
  const items = new Map<string, string>();
  return {
    getItem: (key: string) => items.get(key) ?? null,
    setItem: (key: string, value: string) => void items.set(key, value),
    removeItem: (key: string) => void items.delete(key),
  };
}

function apiReplying(status: number, session: Session) {
  const seen: Request[] = [];
  vi.stubGlobal("fetch", (request: Request) => {
    seen.push(request);
    return Promise.resolve(
      new Response("{}", { status, headers: { "content-type": "application/json" } }),
    );
  });
  return { api: createApi("http://bia.test", session), seen };
}

describe("the API client", () => {
  it("sends the session's Token as a bearer credential", async () => {
    const session = new Session(memoryStorage());
    session.setToken("bia_abc");
    const { api, seen } = apiReplying(200, session);
    await api.GET("/v1/workflows");
    expect(seen[0]?.headers.get("Authorization")).toBe("Bearer bia_abc");
    expect(seen[0]?.url).toBe("http://bia.test/v1/workflows");
  });

  it("ends the session when the API answers 401", async () => {
    const session = new Session(memoryStorage());
    session.setToken("bia_abc");
    const { api } = apiReplying(401, session);
    await api.GET("/v1/workflows");
    expect(session.getToken()).toBeNull();
  });

  it("keeps the session on other failures", async () => {
    const session = new Session(memoryStorage());
    session.setToken("bia_abc");
    const { api } = apiReplying(403, session);
    await api.GET("/v1/workflows");
    expect(session.getToken()).toBe("bia_abc");
  });
});

describe("the session", () => {
  it("restores the Token from storage and forgets it on clear", () => {
    const storage = memoryStorage();
    new Session(storage).setToken("bia_abc");
    const restored = new Session(storage);
    expect(restored.getToken()).toBe("bia_abc");
    restored.clear();
    expect(new Session(storage).getToken()).toBeNull();
  });

  it("works in memory when storage throws", () => {
    const broken = {
      getItem: () => {
        throw new Error("blocked");
      },
      setItem: () => {
        throw new Error("blocked");
      },
      removeItem: () => {
        throw new Error("blocked");
      },
    };
    const session = new Session(broken);
    session.setToken("bia_abc");
    expect(session.getToken()).toBe("bia_abc");
  });
});
