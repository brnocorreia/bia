//! CEL Expressions and Templates (ADR-0004, ADR-0026).
//!
//! An [`Expression`] is compiled once and evaluated against a [`Scope`] as
//! many times as needed. A [`Template`] is a Params string holding
//! `{{ expression }}` segments. Both are pure: evaluation reads only the
//! scope it is given, and every evaluation is bounded by [`Limits`].

mod budget;
mod convert;
mod error;
mod params;
mod references;
mod scope;
mod template;

use std::sync::{Arc, LazyLock};

use cel::common::ast::{EntryExpr, Expr, IdedExpr};
use cel::parser::PrattParser;
use cel::{Context, Env};

pub use error::{CompileError, CompileErrorKind, EvalError};
pub use params::{ParamsError, ResolvedParams, resolve_params};
pub use references::References;
pub use scope::Scope;
pub use template::Template;

/// Bounds on compiling and evaluating one Expression, so a Definition can't
/// run away (ADR-0001).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Longest Expression source accepted, in bytes.
    pub max_source_len: usize,
    /// Deepest syntax tree accepted, counted after macro expansion: `a.b`
    /// is two levels, `[1].map(x, x)` six.
    pub max_nesting_depth: u16,
    /// Cost units one evaluation may spend. Every comprehension iteration
    /// (`map`, `filter`, `all`, `exists`, `exists_one`) costs one unit plus
    /// the size of the value it produces; see ADR-0026.
    pub max_cost: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_source_len: 8 * 1024,
            max_nesting_depth: 32,
            max_cost: 100_000,
        }
    }
}

/// The CEL environment every Expression compiles and evaluates in: the
/// standard library plus the `strings`, `math` and `base64` extensions.
static ENV: LazyLock<Arc<Env>> = LazyLock::new(|| {
    let mut env = Env::stdlib();
    for extension in [
        cel::extensions::strings,
        cel::extensions::math,
        cel::extensions::encoders,
    ] {
        env.add_extension(extension)
            .expect("builtin CEL extensions register on a fresh environment");
    }
    Arc::new(env)
});

/// A compiled CEL Expression.
#[derive(Debug, Clone)]
pub struct Expression {
    source: String,
    program: IdedExpr,
    references: References,
    max_cost: u64,
}

impl Expression {
    /// Compiles `source` with the default [`Limits`].
    pub fn compile(source: &str) -> Result<Self, CompileError> {
        Self::compile_with(source, &Limits::default())
    }

    /// Compiles `source`, rejecting it when it exceeds `limits`.
    pub fn compile_with(source: &str, limits: &Limits) -> Result<Self, CompileError> {
        if source.len() > limits.max_source_len {
            return Err(CompileError::new(
                CompileErrorKind::TooLong,
                format!(
                    "Expression is {} bytes long, more than the limit of {}",
                    source.len(),
                    limits.max_source_len
                ),
                source,
                0,
            ));
        }
        if source.trim().is_empty() {
            return Err(CompileError::new(
                CompileErrorKind::Empty,
                "Expression is empty",
                source,
                0,
            ));
        }
        // The Pratt parser rather than cel's default ANTLR one: ANTLR needs
        // so much stack per nesting level that a dozen parentheses overflow a
        // 2 MiB thread in a debug build, before its own recursion limit.
        let mut program = PrattParser::new()
            .max_recursion_depth(limits.max_nesting_depth)
            .enable_optional_syntax(true)
            .parse(source)
            .map_err(|errors| CompileError::from_parse(source, &errors))?;
        // Evaluation recurses once per level of the syntax tree, so its depth
        // is bounded too, macro expansions included.
        let depth = depth(&program);
        if depth > usize::from(limits.max_nesting_depth) {
            return Err(CompileError::new(
                CompileErrorKind::TooDeep,
                format!(
                    "Expression nests {depth} levels deep, more than the limit of {}",
                    limits.max_nesting_depth
                ),
                source,
                0,
            ));
        }
        let references = References::of(&program);
        budget::instrument(&mut program);
        Ok(Self {
            source: source.to_owned(),
            program,
            references,
            max_cost: limits.max_cost,
        })
    }

    /// The source the Expression was compiled from.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// The `steps.<id>` and `secrets.<NAME>` the Expression reads.
    pub fn references(&self) -> &References {
        &self.references
    }

    /// Evaluates the Expression to a JSON value.
    pub fn evaluate(&self, scope: &Scope) -> Result<serde_json::Value, EvalError> {
        let value = self.evaluate_cel(scope)?;
        convert::to_json(&value).map_err(|reason| EvalError::NotJson {
            reason: scope.redact(&reason),
        })
    }

    /// Evaluates the Expression where a condition is expected (`when`,
    /// `until`, `if`): anything but a boolean is [`EvalError::NotABoolean`].
    pub fn evaluate_condition(&self, scope: &Scope) -> Result<bool, EvalError> {
        match self.evaluate_cel(scope)? {
            cel::Value::Bool(value) => Ok(value),
            other => Err(EvalError::NotABoolean {
                got: other.type_of().to_string(),
            }),
        }
    }

    fn evaluate_cel(&self, scope: &Scope) -> Result<cel::Value, EvalError> {
        let mut context = Context::with_env(Arc::clone(&ENV));
        scope.bind(&mut context);
        let meter = budget::Meter::new(self.max_cost);
        meter.bind(&mut context);
        let result = cel::Value::resolve(&self.program, &context);
        // Checked first: an outer `||` or `&&` can absorb the limit error,
        // and the Expression must still fail once it ran out of budget.
        if meter.exceeded() {
            return Err(EvalError::CostLimitExceeded {
                limit: self.max_cost,
            });
        }
        result.map_err(|error| EvalError::from_execution(error, scope))
    }
}

/// The number of levels in the syntax tree under `expr`, counted without
/// recursing, since the tree is not yet known to be shallow.
fn depth(expr: &IdedExpr) -> usize {
    let mut deepest = 0;
    let mut pending = vec![(expr, 1)];
    while let Some((expr, level)) = pending.pop() {
        deepest = deepest.max(level);
        let children: Vec<&IdedExpr> = match &expr.expr {
            Expr::Call(call) => call
                .target
                .as_deref()
                .into_iter()
                .chain(&call.args)
                .collect(),
            Expr::Comprehension(c) => vec![
                &c.iter_range,
                &c.accu_init,
                &c.loop_cond,
                &c.loop_step,
                &c.result,
            ],
            Expr::List(list) => list.elements.iter().collect(),
            Expr::Map(map) => map
                .entries
                .iter()
                .filter_map(|entry| match &entry.expr {
                    EntryExpr::MapEntry(entry) => Some([&entry.key, &entry.value]),
                    EntryExpr::StructField(_) => None,
                })
                .flatten()
                .collect(),
            Expr::Struct(strct) => strct
                .entries
                .iter()
                .filter_map(|entry| match &entry.expr {
                    EntryExpr::StructField(field) => Some(&field.value),
                    EntryExpr::MapEntry(_) => None,
                })
                .collect(),
            Expr::Select(select) => vec![&*select.operand],
            Expr::Ident(_) | Expr::Literal(_) | Expr::Unspecified => Vec::new(),
        };
        pending.extend(children.into_iter().map(|child| (child, level + 1)));
    }
    deepest
}
