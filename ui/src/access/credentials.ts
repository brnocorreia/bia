import type { TokenInfo } from "@/api/tokens";

export type Credentials = {
  sessions: TokenInfo[];
  /** `oauth` Tokens, which Grants will group by `grant_id` (BIA-95). */
  oauth: TokenInfo[];
  manual: TokenInfo[];
};

/** Splits the API's flat list by `origin`, keeping its order. */
export function groupByOrigin(tokens: TokenInfo[]): Credentials {
  return {
    sessions: tokens.filter((t) => t.origin === "session"),
    oauth: tokens.filter((t) => t.origin === "oauth"),
    manual: tokens.filter((t) => t.origin === "manual"),
  };
}

const BROWSERS: [RegExp, string][] = [
  [/Edg\//, "Edge"],
  [/OPR\/|Opera/, "Opera"],
  [/Firefox\//, "Firefox"],
  [/Chrome\/|Chromium\//, "Chrome"],
  [/Safari\//, "Safari"],
];

const SYSTEMS: [RegExp, string][] = [
  [/Android/, "Android"],
  [/iPhone|iPad|iOS/, "iOS"],
  [/Windows/, "Windows"],
  [/Mac OS X|Macintosh/, "macOS"],
  [/Linux|X11/, "Linux"],
];

/** A short label for a Session's `User-Agent`, e.g. "Firefox on Linux". */
export function describeBrowser(userAgent: string | null | undefined): string {
  if (!userAgent) return "Unknown browser";
  const browser = BROWSERS.find(([pattern]) => pattern.test(userAgent))?.[1];
  const system = SYSTEMS.find(([pattern]) => pattern.test(userAgent))?.[1];
  if (browser && system) return `${browser} on ${system}`;
  return browser ?? userAgent.slice(0, 60);
}
