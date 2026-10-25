const KEY = "bia.pollIntervalMs";
const DEFAULT_MS = 2000;

/**
 * How often an active Run (and a list showing one) is refetched. A browser can override it
 * through `localStorage["bia.pollIntervalMs"]`, which is how the end-to-end tests stay fast.
 * SSE replaces polling later (BIA-88).
 */
export function pollIntervalMs(): number {
  try {
    const stored = Number(window.localStorage.getItem(KEY));
    if (Number.isFinite(stored) && stored >= 50) return stored;
  } catch {
    // Storage can be blocked; the default is fine.
  }
  return DEFAULT_MS;
}
