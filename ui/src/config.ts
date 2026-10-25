export interface AppConfig {
  /** Origin of the bia API. Empty means the page's own origin, as when the API serves the UI. */
  apiUrl: string;
}

export const DEFAULT_CONFIG: AppConfig = { apiUrl: "http://127.0.0.1:8080" };

export function parseConfig(raw: unknown): AppConfig {
  if (
    typeof raw !== "object" ||
    raw === null ||
    !("apiUrl" in raw) ||
    typeof raw.apiUrl !== "string"
  ) {
    throw new Error('config.json must be an object with a string "apiUrl"');
  }
  return { apiUrl: raw.apiUrl.replace(/\/+$/, "") };
}

let current: AppConfig = DEFAULT_CONFIG;

export function setConfig(config: AppConfig) {
  current = config;
}

export function getConfig(): AppConfig {
  return current;
}

/**
 * Read at startup instead of baked in at build time, so one build serves every environment.
 * A missing file means local development; a present but malformed one is a deployment mistake
 * and fails loudly instead of silently talking to the wrong API.
 */
export async function loadConfig(
  fetchConfig: typeof fetch = fetch,
  url = "/config.json",
): Promise<AppConfig> {
  const response = await fetchConfig(url, { cache: "no-store" });
  if (response.status === 404) return DEFAULT_CONFIG;
  if (!response.ok) throw new Error(`cannot load ${url}: HTTP ${response.status}`);
  const type = response.headers.get("content-type") ?? "";
  // A static host's SPA fallback answers a missing file with index.html.
  if (!type.includes("json")) return DEFAULT_CONFIG;
  return parseConfig(await response.json());
}
