import { useSyncExternalStore } from "react";

export type SessionState = "unknown" | "signed-in" | "signed-out";

/**
 * What the UI last learned about its Session. The Session itself is an `HttpOnly` cookie the
 * API sets, so no script can read or keep it (ADR-0064); this only remembers whether the API
 * still accepts it, so the shell knows when to send the user back to sign-in.
 */
export class Session {
  private state: SessionState = "unknown";
  private listeners = new Set<() => void>();

  getState = (): SessionState => this.state;

  signedIn() {
    this.set("signed-in");
  }

  signedOut() {
    this.set("signed-out");
  }

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  private set(state: SessionState) {
    if (this.state === state) return;
    this.state = state;
    this.listeners.forEach((listener) => listener());
  }
}

export const session = new Session();

export function useSessionState() {
  return useSyncExternalStore(session.subscribe, session.getState);
}
