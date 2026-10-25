import EditorWorker from "monaco-editor/esm/vs/editor/editor.worker?worker";
import JsonWorker from "monaco-editor/esm/vs/language/json/json.worker?worker";
import YamlWorker from "./yaml.worker?worker";

// Must run before Monaco loads, so it is its own module. Monaco and its workers are bundled with the app (no CDN loader), so the editor works under
// the embedded UI's CSP (`script-src 'self'`, `worker-src 'self' blob:`; ADR-0050, ADR-0055).
globalThis.MonacoEnvironment = {
  getWorker(_id: string, label: string) {
    if (label === "json") return new JsonWorker();
    if (label === "yaml") return new YamlWorker();
    return new EditorWorker();
  },
};
