import js from "@eslint/js";
import reactHooks from "eslint-plugin-react-hooks";
import globals from "globals";
import tseslint from "typescript-eslint";

export default tseslint.config(
  { ignores: ["dist", "playwright-report", "test-results", "src/**/*.gen.ts"] },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  { files: ["src/**"], ...reactHooks.configs.flat.recommended },
  {
    languageOptions: { globals: { ...globals.browser, ...globals.node } },
    rules: {
      "@typescript-eslint/no-unused-vars": ["error", { argsIgnorePattern: "^_" }],
    },
  },
  {
    // Playwright fixtures must destructure their dependencies, even when there are none.
    files: ["e2e/**"],
    rules: { "no-empty-pattern": "off" },
  },
);
