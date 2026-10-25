//! Human-readable output: Diagnostics, Timelines, tables.

use std::fmt::Write;

use bia_client::{AttemptStatus, TimelineEntry};
use bia_core::validation::{Diagnostic, Severity};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::Value;

use super::source::Source;

/// Appends each Diagnostic as `file:line: severity[code] /path`, its message and its
/// suggestion. `source` gives the file and, for YAML, the line.
pub(crate) fn diagnostics(out: &mut String, source: Option<&Source>, found: &[Diagnostic]) {
    for diagnostic in found {
        let severity = match diagnostic.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        let path = if diagnostic.path.is_empty() {
            "/"
        } else {
            &diagnostic.path
        };
        let place = source.map(|source| {
            let file = source.path.display();
            match source.line_of(&diagnostic.path) {
                Some(line) => format!("{file}:{line}: "),
                None => format!("{file}: "),
            }
        });
        let _ = writeln!(
            out,
            "{}{severity}[{}] {path}",
            place.unwrap_or_default(),
            diagnostic.code
        );
        let _ = writeln!(out, "    {}", diagnostic.message);
        if let Some(suggestion) = &diagnostic.suggestion {
            let _ = writeln!(out, "    suggestion: {suggestion}");
        }
    }
}

pub(crate) fn error_count(found: &[Diagnostic]) -> usize {
    found
        .iter()
        .filter(|diagnostic| diagnostic.is_error())
        .count()
}

pub(crate) fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("{count} {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

pub(crate) fn time(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// Left-aligned columns separated by two spaces.
pub(crate) fn table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let widths: Vec<usize> = (0..headers.len())
        .map(|column| {
            rows.iter()
                .map(|row| row[column].chars().count())
                .chain([headers[column].chars().count()])
                .max()
                .unwrap_or_default()
        })
        .collect();
    let mut out = String::new();
    let mut line = |cells: &mut dyn Iterator<Item = &str>| {
        let mut text = String::new();
        for (cell, width) in cells.zip(&widths) {
            let _ = write!(text, "{cell:<width$}  ");
        }
        out.push_str(text.trim_end());
        out.push('\n');
    };
    line(&mut headers.iter().copied());
    for row in rows {
        line(&mut row.iter().map(String::as_str));
    }
    out
}

fn status(status: AttemptStatus) -> &'static str {
    match status {
        AttemptStatus::Succeeded => "succeeded",
        AttemptStatus::Failed => "failed",
        AttemptStatus::Skipped => "skipped",
        AttemptStatus::Cancelled => "cancelled",
    }
}

fn duration(ms: i64) -> String {
    if ms < 1_000 {
        format!("{ms}ms")
    } else {
        format!("{:.1}s", ms as f64 / 1_000.0)
    }
}

/// `value` as compact JSON, cut to a line a terminal can show.
fn brief(value: &Value) -> String {
    const LIMIT: usize = 120;
    let text = value.to_string();
    if text.chars().count() <= LIMIT {
        return text;
    }
    let cut: String = text.chars().take(LIMIT).collect();
    format!("{cut}...")
}

/// A Timeline as a table, with each entry's Output or error on the lines under it.
pub(crate) fn timeline(entries: &[TimelineEntry]) -> String {
    if entries.is_empty() {
        return "no Timeline entries\n".to_owned();
    }
    let rows: Vec<Vec<String>> = entries
        .iter()
        .map(|entry| {
            let step = match (&entry.step_id, &entry.iteration_path) {
                (Some(step), Some(path)) => format!("{step} [{path}]"),
                (Some(step), None) => step.clone(),
                (None, _) => "(run)".to_owned(),
            };
            vec![
                entry.seq.to_string(),
                step,
                entry.attempt.map(|a| a.to_string()).unwrap_or_default(),
                entry.action.clone().unwrap_or_default(),
                status(entry.status).to_owned(),
                duration(entry.duration_ms),
            ]
        })
        .collect();
    let table = table(
        &["SEQ", "STEP", "ATTEMPT", "ACTION", "STATUS", "TOOK"],
        &rows,
    );
    let mut out = String::new();
    let mut table_lines = table.lines();
    out.push_str(table_lines.next().unwrap_or_default());
    out.push('\n');
    for (entry, line) in entries.iter().zip(table_lines) {
        out.push_str(line);
        out.push('\n');
        if let Some(error) = &entry.error {
            let _ = writeln!(out, "    error: {}", brief(error));
        } else if let Some(output) = &entry.output {
            let _ = writeln!(out, "    output: {}", brief(output));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::table;

    #[test]
    fn table_pads_columns_to_their_widest_cell() {
        let rows = vec![
            vec!["a".to_owned(), "long cell".to_owned()],
            vec!["bbb".to_owned(), "x".to_owned()],
        ];
        assert_eq!(
            table(&["ID", "NAME"], &rows),
            "ID   NAME\na    long cell\nbbb  x\n"
        );
    }
}
