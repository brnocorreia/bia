import { defineConfig } from "@playwright/test";
import { e2eEnv } from "./e2e/support/env";

const { uiUrl } = e2eEnv();

// `global-setup` starts a real `bia serve` against its own database and serves the built
// bundle; see `ui/README.md` for the environment variables. The `setup` project then creates
// the admin through the Setup Link, which every other test depends on.
export default defineConfig({
  testDir: "./e2e",
  globalSetup: "./e2e/support/global-setup.ts",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  reporter: process.env.CI ? [["github"], ["html", { open: "never" }]] : "list",
  use: { baseURL: uiUrl, trace: "retain-on-failure" },
  projects: [
    { name: "setup", testMatch: /\.setup\.ts$/, use: { browserName: "chromium" } },
    {
      name: "chromium",
      dependencies: ["setup"],
      testIgnore: /account\//,
      use: { browserName: "chromium" },
    },
    // Changing the password signs other Sessions out, so it runs once the rest are done.
    {
      name: "account",
      dependencies: ["chromium"],
      testMatch: /account\/.*\.spec\.ts$/,
      use: { browserName: "chromium" },
    },
  ],
});
