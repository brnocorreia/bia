import { useSyncExternalStore } from "react";

const STORAGE_KEY = "bia.token";

/**
 * The Token the UI sends as `Authorization: Bearer`. It lives in `sessionStorage`, so it
 * survives a reload but not closing the tab, and is never shared with other tabs (ADR-0050).
 */
export class Session {
  private token: string | null;
  private listeners = new Set<() => void>();

  constructor(private storage: Pick<Storage, "getItem" | "setItem" | "removeItem"> | null) {
    this.token = this.read();
  }

  getToken = (): string | null => this.token;

  setToken(token: string) {
    this.token = token;
    this.write(() => this.storage?.setItem(STORAGE_KEY, token));
    this.notify();
  }

  clear() {
    if (this.token === null) return;
    this.token = null;
    this.write(() => this.storage?.removeItem(STORAGE_KEY));
    this.notify();
  }

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  // Storage can be missing or throw (blocked site data, private windows); the Token then lives
  // in memory only and the user logs in again after a reload.
  private read() {
    try {
      return this.storage?.getItem(STORAGE_KEY) ?? null;
    } catch {
      return null;
    }
  }

  private write(action: () => void) {
    try {
      action();
    } catch {
      /* memory only */
    }
  }

  private notify() {
    this.listeners.forEach((listener) => listener());
  }
}

function browserStorage() {
  try {
    return window.sessionStorage;
  } catch {
    return null;
  }
}

export const session = new Session(browserStorage());

export function useToken() {
  return useSyncExternalStore(session.subscribe, session.getToken);
}
