import { execFileSync, spawn, type ChildProcess } from "node:child_process";
import { writeFileSync } from "node:fs";
import path from "node:path";
import { preview } from "vite";
import pg from "pg";
import { e2eEnv, type E2eEnv } from "./env";

const uiRoot = path.resolve(import.meta.dirname, "../..");
const repoRoot = path.resolve(uiRoot, "..");

async function recreateDatabase(databaseUrl: string) {
  const url = new URL(databaseUrl);
  const name = decodeURIComponent(url.pathname.slice(1));
  // This drops the database, so refuse anything that could be a real one.
  if (!name.startsWith("bia_e2e")) {
    throw new Error(
      `BIA_E2E_DATABASE_URL must name a database starting with bia_e2e, got "${name}"`,
    );
  }
  url.pathname = "/postgres";
  const admin = new pg.Client({ connectionString: url.toString() });
  await admin.connect();
  try {
    await admin.query(`DROP DATABASE IF EXISTS "${name}" WITH (FORCE)`);
    await admin.query(`CREATE DATABASE "${name}"`);
  } finally {
    await admin.end();
  }
}

function binaryPath(env: E2eEnv) {
  if (env.binary) return env.binary;
  const features = env.embedded ? ["--features", "ui"] : [];
  execFileSync("cargo", ["build", "-p", "bia-cli", ...features], {
    cwd: repoRoot,
    stdio: "inherit",
    env: { ...process.env, SQLX_OFFLINE: "true" },
  });
  return path.join(process.env.CARGO_TARGET_DIR ?? path.join(repoRoot, "target"), "debug/bia");
}

async function waitUntilReady(url: string, server: ChildProcess) {
  for (let attempt = 0; attempt < 150; attempt++) {
    if (server.exitCode !== null) throw new Error(`bia serve exited with ${server.exitCode}`);
    try {
      if ((await fetch(`${url}/ready`)).ok) return;
    } catch {
      /* not listening yet */
    }
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
  throw new Error(`bia serve did not become ready at ${url}`);
}

export default async function globalSetup() {
  const env = e2eEnv();
  await recreateDatabase(env.databaseUrl);

  const bia = binaryPath(env);
  const serverEnv = {
    ...process.env,
    DATABASE_URL: env.databaseUrl,
    BIA_API_BIND: `127.0.0.1:${env.apiPort}`,
    BIA_BOOTSTRAP_KEY: env.bootstrapKey,
    BIA_CORS_ORIGINS: env.uiUrl,
  };
  execFileSync(bia, ["migrate"], { env: serverEnv, stdio: "inherit" });
  const server = spawn(bia, ["serve"], { env: serverEnv, stdio: "inherit" });
  await waitUntilReady(env.apiUrl, server);

  const bootstrap = await fetch(`${env.apiUrl}/v1/tokens/bootstrap`, {
    method: "POST",
    headers: {
      Authorization: `Bearer ${env.bootstrapKey}`,
      "Content-Type": "application/json",
    },
    body: JSON.stringify({ name: "e2e-admin" }),
  });
  if (!bootstrap.ok)
    throw new Error(`bootstrap failed: ${bootstrap.status} ${await bootstrap.text()}`);
  // Workers inherit this, and fixtures read it.
  process.env.BIA_E2E_ADMIN_TOKEN = ((await bootstrap.json()) as { value: string }).value;

  // The binary serves the UI itself and its own `config.json`.
  if (env.embedded) return async () => void server.kill("SIGTERM");

  // The bundle is served as built; only `config.json` is written, which is what a deployment does.
  writeFileSync(path.join(uiRoot, "dist/config.json"), JSON.stringify({ apiUrl: env.apiUrl }));
  const ui = await preview({
    root: uiRoot,
    preview: { host: "127.0.0.1", port: env.uiPort, strictPort: true },
  });

  return async () => {
    await ui.close();
    server.kill("SIGTERM");
  };
}
