import { randomBytes } from "node:crypto";
import path from "node:path";

export interface E2eEnv {
  /** Port of the `bia serve` the tests start. */
  apiPort: number;
  apiUrl: string;
  /** Port of the static server for the built bundle. */
  uiPort: number;
  uiUrl: string;
  /** A database whose name starts with `bia_e2e`: it is dropped and recreated on every run. */
  databaseUrl: string;
  /** The `bia` binary; when unset the setup builds `bia-cli` and uses `target/debug/bia`. */
  binary: string | undefined;
  /** Where the setup project leaves the admin Token setup returned, for the other tests. */
  adminTokenFile: string;
  /** The Secrets master key the server seals values under: fresh per call, since the database is recreated each run and nothing must outlive it. */
  masterKey: string;
  /** Test the UI embedded in `bia` (built with `--features ui`) instead of the static bundle. */
  embedded: boolean;
}

export function e2eEnv(): E2eEnv {
  const apiPort = Number(process.env.BIA_E2E_PORT ?? 18080);
  const uiPort = Number(process.env.BIA_E2E_UI_PORT ?? apiPort + 1000);
  const embedded = process.env.BIA_E2E_EMBEDDED === "1";
  return {
    embedded,
    apiPort,
    apiUrl: `http://127.0.0.1:${apiPort}`,
    uiPort,
    uiUrl: embedded ? `http://127.0.0.1:${apiPort}` : `http://127.0.0.1:${uiPort}`,
    databaseUrl: process.env.BIA_E2E_DATABASE_URL ?? "postgres://bia:bia@localhost:5432/bia_e2e",
    binary: process.env.BIA_E2E_BIN,
    adminTokenFile: path.resolve(import.meta.dirname, "../.auth/admin-token"),
    masterKey: randomBytes(32).toString("hex"),
  };
}
