//! Maps a Diagnostic's JSON Pointer back to a line of the YAML file it came from.
//!
//! The server and `bia-core` only see canonical JSON, so the position is recovered here by
//! walking the file's indentation. It understands block-style YAML, which is what Definitions
//! are written in; for anything else (flow collections, anchors that move keys) it returns the
//! line of the deepest ancestor it could find, or `None`.

use std::ops::Range;

struct Line {
    number: usize,
    /// Columns before the line's first character.
    leading: usize,
    /// Columns before the text left after the sequence markers (`- `).
    indent: usize,
    text: String,
    item: bool,
}

/// The lines holding a node's children.
struct Content {
    range: Range<usize>,
    /// Whether the first line is a sequence item that already holds the node's first key
    /// (`- id: a`), as opposed to a line that opens the node (`steps:`) or its first item.
    from_item: bool,
}

/// The 1-based line of the value at `pointer` in `yaml`, or of its nearest ancestor.
pub(crate) fn line_of(yaml: &str, pointer: &str) -> Option<usize> {
    let lines = lines(yaml);
    let mut content = Content {
        range: 0..lines.len(),
        from_item: true,
    };
    let mut found = None;
    for segment in pointer.split('/').skip(1) {
        let segment = segment.replace("~1", "/").replace("~0", "~");
        let Some((line, next)) = child(&lines, &content, &segment) else {
            break;
        };
        found = Some(lines[line].number);
        content = next;
    }
    found
}

fn lines(yaml: &str) -> Vec<Line> {
    let mut out = Vec::new();
    for (index, original) in yaml.lines().enumerate() {
        let trimmed = original.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed == "---" {
            continue;
        }
        let leading = original.len() - trimmed.len();
        let mut indent = leading;
        let mut text = trimmed;
        let mut item = false;
        while let Some(rest) = text.strip_prefix("- ") {
            item = true;
            let after = rest.trim_start();
            indent += text.len() - after.len();
            text = after;
        }
        out.push(Line {
            number: index + 1,
            leading,
            indent,
            text: text.to_owned(),
            item,
        });
    }
    out
}

fn child(lines: &[Line], content: &Content, segment: &str) -> Option<(usize, Content)> {
    let range = &content.range;
    let first = lines.get(range.start).filter(|_| range.start < range.end)?;
    if first.item && !content.from_item {
        let index: usize = segment.parse().ok()?;
        let heads: Vec<usize> = range
            .clone()
            .filter(|&at| lines[at].item && lines[at].leading == first.leading)
            .collect();
        let line = *heads.get(index)?;
        let end = heads.get(index + 1).copied().unwrap_or(range.end);
        return Some((
            line,
            Content {
                range: line..end,
                from_item: true,
            },
        ));
    }
    let base = first.indent;
    let line = range.clone().find(|&at| {
        lines[at].indent == base && key_of(&lines[at].text).as_deref() == Some(segment)
    })?;
    // A sequence may be written at its key's indentation, so such items stay inside the key.
    let end = (line + 1..range.end)
        .find(|&at| lines[at].leading <= base && !(lines[at].item && lines[at].leading == base))
        .unwrap_or(range.end);
    Some((
        line,
        Content {
            range: line + 1..end,
            from_item: false,
        },
    ))
}

fn key_of(text: &str) -> Option<String> {
    let (key, rest) = if let Some(quoted) = text.strip_prefix('"') {
        let end = quoted.find('"')?;
        (&quoted[..end], &quoted[end + 1..])
    } else if let Some(quoted) = text.strip_prefix('\'') {
        let end = quoted.find('\'')?;
        (&quoted[..end], &quoted[end + 1..])
    } else {
        let end = text.find(':')?;
        (&text[..end], &text[end..])
    };
    (rest.starts_with(':') && (rest.len() == 1 || rest[1..].starts_with(' ')))
        .then(|| key.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::line_of;

    const YAML: &str = "\
schema_version: 1
id: price
# a comment
steps:
  - id: total
    action: transform.expr
    params:
      expression: values.price
  - id: done
    action: flow.succeed
";

    #[test]
    fn finds_keys_and_sequence_items() {
        assert_eq!(line_of(YAML, "/id"), Some(2));
        assert_eq!(line_of(YAML, "/steps"), Some(4));
        assert_eq!(line_of(YAML, "/steps/0"), Some(5));
        assert_eq!(line_of(YAML, "/steps/0/action"), Some(6));
        assert_eq!(line_of(YAML, "/steps/0/params/expression"), Some(8));
        assert_eq!(line_of(YAML, "/steps/1/action"), Some(10));
    }

    #[test]
    fn falls_back_to_the_nearest_ancestor() {
        assert_eq!(line_of(YAML, "/steps/0/retry"), Some(5));
        assert_eq!(line_of(YAML, "/steps/7/action"), Some(4));
        assert_eq!(line_of(YAML, "/nope"), None);
    }

    #[test]
    fn the_root_has_no_line() {
        assert_eq!(line_of(YAML, ""), None);
    }

    #[test]
    fn reads_sequences_written_at_the_keys_indentation() {
        let yaml = "steps:\n- id: a\n  action: x\n- id: b\n  action: y\n";
        assert_eq!(line_of(yaml, "/steps/1/action"), Some(5));
    }
}
