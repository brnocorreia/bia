//! Definition files and the JSON or YAML documents (input, Mocks) the commands read.

use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use bia_core::dsl::yaml_to_json;
use serde_json::Value;

use super::locate;

/// A Definition file, converted to the JSON the server accepts.
pub(crate) struct Source {
    pub(crate) path: PathBuf,
    pub(crate) value: Value,
    /// The YAML text, kept to point Diagnostics at its lines.
    yaml: Option<String>,
}

impl Source {
    /// The Workflow id the file declares.
    pub(crate) fn workflow_id(&self) -> &str {
        self.value
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    /// The line of the value a Diagnostic's `pointer` names, for YAML files.
    pub(crate) fn line_of(&self, pointer: &str) -> Option<usize> {
        locate::line_of(self.yaml.as_deref()?, pointer)
    }

    fn read(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        let (value, yaml) = if is_yaml(path) {
            let value = yaml_to_json(&text)
                .map_err(|error| anyhow::anyhow!("{}: invalid YAML: {error}", path.display()))?;
            (value, Some(text))
        } else {
            let value = serde_json::from_str(&text)
                .map_err(|error| anyhow::anyhow!("{}: invalid JSON: {error}", path.display()))?;
            (value, None)
        };
        if let Some(kind) = value.get("kind") {
            bail!(
                "{}: `kind: {}` resources cannot be applied yet; only Workflow Definitions can",
                path.display(),
                kind.as_str().unwrap_or("?")
            );
        }
        let source = Self {
            path: path.to_owned(),
            value,
            yaml,
        };
        if source.workflow_id().is_empty() {
            bail!("{}: the Definition has no string `id`", path.display());
        }
        Ok(source)
    }
}

fn is_yaml(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("yaml" | "yml")
    )
}

fn is_definition_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("json" | "yaml" | "yml")
    )
}

/// Reads every file named in `paths`, and every `.json`, `.yaml` and `.yml` file below each
/// directory, in path order. Nothing is returned unless all of them parse.
pub(crate) fn collect(paths: &[PathBuf]) -> anyhow::Result<Vec<Source>> {
    let mut files = Vec::new();
    for path in paths {
        if path.is_dir() {
            walk(path, &mut files)?;
        } else if path.is_file() {
            files.push(path.clone());
        } else {
            bail!("{} does not exist", path.display());
        }
    }
    if files.is_empty() {
        bail!("no .json, .yaml or .yml files found");
    }
    files.sort();
    files.dedup();
    files.iter().map(|path| Source::read(path)).collect()
}

fn walk(dir: &Path, files: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    let entries =
        std::fs::read_dir(dir).with_context(|| format!("cannot read {}", dir.display()))?;
    for entry in entries {
        let path = entry?.path();
        let hidden = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with('.'));
        if hidden {
            continue;
        }
        if path.is_dir() {
            walk(&path, files)?;
        } else if is_definition_file(&path) {
            files.push(path);
        }
    }
    Ok(())
}

/// A JSON or YAML document from `path`, or from standard input for `-`.
pub(crate) fn document(path: &str) -> anyhow::Result<Value> {
    let (text, yaml) = if path == "-" {
        let mut text = String::new();
        std::io::stdin()
            .read_to_string(&mut text)
            .context("cannot read standard input")?;
        (text, false)
    } else {
        let text = std::fs::read_to_string(path).with_context(|| format!("cannot read {path}"))?;
        (text, is_yaml(Path::new(path)))
    };
    // Standard input has no extension to go by, so anything that is not JSON is read as YAML.
    if !yaml && let Ok(value) = serde_json::from_str(&text) {
        return Ok(value);
    }
    if yaml || path == "-" {
        return yaml_to_json(&text)
            .map_err(|error| anyhow::anyhow!("{path}: invalid YAML: {error}"));
    }
    serde_json::from_str(&text).map_err(|error| anyhow::anyhow!("{path}: invalid JSON: {error}"))
}
