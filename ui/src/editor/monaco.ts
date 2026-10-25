import "./environment";
import * as monaco from "monaco-editor";
import { configureMonacoYaml } from "monaco-yaml";
const jsonDefaults = monaco.languages.json.jsonDefaults;

const SCHEMA_URI = "bia://schema/definition.json";

/** Model URIs the schema is attached to; one per format, so the editor can swap models. */
export const MODEL_URI = {
  json: monaco.Uri.parse("bia://editor/definition.json"),
  yaml: monaco.Uri.parse("bia://editor/definition.yaml"),
};

let yaml: ReturnType<typeof configureMonacoYaml> | null = null;

/**
 * Points completion and hover at the Definition's JSON Schema. Client-side validation is off:
 * the server's Diagnostics are the single source of truth, so a problem is never underlined
 * twice with different wording.
 */
export function setDefinitionSchema(schema: object) {
  jsonDefaults.setDiagnosticsOptions({
    validate: false,
    allowComments: false,
    enableSchemaRequest: false,
    schemas: [{ uri: SCHEMA_URI, fileMatch: ["definition.json"], schema }],
  });
  const options = {
    validate: false,
    completion: true,
    hover: true,
    schemas: [{ uri: SCHEMA_URI, fileMatch: ["**/definition.yaml"], schema }],
  };
  if (yaml) yaml.update(options);
  else yaml = configureMonacoYaml(monaco, { enableSchemaRequest: false, ...options });
}

export { monaco };
