use super::error::{CompileError, CompileErrorKind, EvalError};
use super::references::References;
use super::scope::Scope;
use super::{Expression, Limits};

/// A Params string with `{{ expression }}` segments.
///
/// A Template that is exactly one segment, with nothing around it, evaluates
/// to the Expression's value and keeps its type: `"{{ input.user }}"` is an
/// object. Any other Template renders a string, where string values are
/// inserted as they are and every other value as compact JSON. A string with
/// no segment is a literal.
///
/// The text between `{{` and `}}` is trimmed and compiled as CEL. The closing
/// `}}` is the first one outside a CEL string literal and outside braces the
/// Expression opened, so `{{ {'a': {'b': 1}} }}` is one map. There is no
/// escape syntax: a literal `{{` is written `{{ '{{' }}`.
#[derive(Debug, Clone)]
pub struct Template {
    source: String,
    parts: Vec<Part>,
    references: References,
}

#[derive(Debug, Clone)]
enum Part {
    Text(String),
    Expression(Expression),
}

impl Template {
    pub fn parse(source: &str) -> Result<Self, CompileError> {
        Self::parse_with(source, &Limits::default())
    }

    /// Parses `source`, compiling every segment with `limits`. Error
    /// positions are relative to the whole Template.
    pub fn parse_with(source: &str, limits: &Limits) -> Result<Self, CompileError> {
        let mut parts = Vec::new();
        let mut text = String::new();
        let mut references = References::default();
        let mut rest = 0;
        while let Some(found) = source[rest..].find("{{") {
            let open = rest + found;
            text.push_str(&source[rest..open]);
            let start = open + 2;
            let end = closing(source, start).ok_or_else(|| {
                CompileError::new(
                    CompileErrorKind::UnclosedTemplate,
                    "'{{' is never closed by '}}'",
                    source,
                    open,
                )
            })?;
            let inner = &source[start..end];
            let trimmed = inner.trim();
            if trimmed.is_empty() {
                return Err(CompileError::new(
                    CompileErrorKind::Empty,
                    "'{{ }}' holds no Expression",
                    source,
                    open,
                ));
            }
            let lead = inner.len() - inner.trim_start().len();
            let expression = Expression::compile_with(trimmed, limits)
                .map_err(|error| error.shifted(source, start + lead))?;
            references.extend(expression.references());
            if !text.is_empty() {
                parts.push(Part::Text(std::mem::take(&mut text)));
            }
            parts.push(Part::Expression(expression));
            rest = end + 2;
        }
        text.push_str(&source[rest..]);
        if !text.is_empty() {
            parts.push(Part::Text(text));
        }
        Ok(Self {
            source: source.to_owned(),
            parts,
            references,
        })
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    /// The `steps.<id>` and `secrets.<NAME>` any segment reads.
    pub fn references(&self) -> &References {
        &self.references
    }

    pub fn is_single_expression(&self) -> bool {
        matches!(self.parts.as_slice(), [Part::Expression(_)])
    }

    pub fn evaluate(&self, scope: &Scope) -> Result<serde_json::Value, EvalError> {
        if let [Part::Expression(expression)] = self.parts.as_slice() {
            return expression.evaluate(scope);
        }
        let mut rendered = String::new();
        for part in &self.parts {
            match part {
                Part::Text(text) => rendered.push_str(text),
                Part::Expression(expression) => match expression.evaluate(scope)? {
                    serde_json::Value::String(s) => rendered.push_str(&s),
                    other => rendered.push_str(&other.to_string()),
                },
            }
        }
        Ok(serde_json::Value::String(rendered))
    }
}

/// The byte offset of the `}}` closing a segment whose Expression starts at
/// `start`, skipping CEL string literals and braces the Expression opens.
fn closing(source: &str, start: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut depth = 0usize;
    let mut i = start;
    while i < bytes.len() {
        match bytes[i] {
            quote @ (b'\'' | b'"') => {
                let raw = i > start && matches!(bytes[i - 1], b'r' | b'R');
                i = string_end(bytes, i, quote, raw)?;
                continue;
            }
            b'{' => depth += 1,
            b'}' if depth > 0 => depth -= 1,
            b'}' if bytes.get(i + 1) == Some(&b'}') => return Some(i),
            _ => {}
        }
        i += 1;
    }
    None
}

fn string_end(bytes: &[u8], open: usize, quote: u8, raw: bool) -> Option<usize> {
    let triple = bytes.get(open..open + 3) == Some(&[quote, quote, quote]);
    let delimiter = if triple { 3 } else { 1 };
    let mut i = open + delimiter;
    while i < bytes.len() {
        if bytes[i] == b'\\' && !raw {
            i += 2;
            continue;
        }
        if bytes[i] == quote && (!triple || bytes.get(i..i + 3) == Some(&[quote, quote, quote])) {
            return Some(i + delimiter);
        }
        i += 1;
    }
    None
}
