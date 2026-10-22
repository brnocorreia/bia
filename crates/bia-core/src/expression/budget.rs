//! Bounded evaluation: comprehensions are the only way a CEL Expression can do
//! work beyond its own size, so every comprehension iteration is charged to a
//! per-evaluation [`Meter`].
//!
//! cel-rust has no cost tracking of its own. After parsing, [`instrument`]
//! wraps one sub-expression of each comprehension in a call to [`CHARGE`],
//! which CEL source cannot name because of the `@`. The call returns its
//! argument unchanged after charging for it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use cel::common::ast::{CallExpr, ComprehensionExpr, EntryExpr, Expr, IdedExpr, operators};
use cel::extractors::Arguments;
use cel::{Context, ExecutionError, Value};

const CHARGE: &str = "@bia_charge";

const BYTES_PER_UNIT: u64 = 64;

/// Rewrites `expr` so that every comprehension iteration calls [`CHARGE`].
pub(super) fn instrument(expr: &mut IdedExpr) {
    match &mut expr.expr {
        Expr::Call(call) => {
            if let Some(target) = &mut call.target {
                instrument(target);
            }
            call.args.iter_mut().for_each(instrument);
        }
        Expr::Comprehension(comprehension) => {
            instrument(&mut comprehension.iter_range);
            instrument(&mut comprehension.accu_init);
            instrument(&mut comprehension.loop_cond);
            instrument(&mut comprehension.loop_step);
            instrument(&mut comprehension.result);
            charge_iterations(comprehension);
        }
        Expr::List(list) => list.elements.iter_mut().for_each(instrument),
        Expr::Map(map) => {
            for entry in &mut map.entries {
                if let EntryExpr::MapEntry(entry) = &mut entry.expr {
                    instrument(&mut entry.key);
                    instrument(&mut entry.value);
                }
            }
        }
        Expr::Struct(strct) => {
            for entry in &mut strct.entries {
                if let EntryExpr::StructField(field) = &mut entry.expr {
                    instrument(&mut field.value);
                }
            }
        }
        Expr::Select(select) => instrument(&mut select.operand),
        Expr::Ident(_) | Expr::Literal(_) | Expr::Unspecified => {}
    }
}

/// Picks the sub-expression evaluated once per iteration that can be wrapped
/// without changing the comprehension's meaning, or the interpreter's fast
/// paths for the standard macros:
///
/// - `all` / `exists` test a non-literal `loop_cond` every iteration, and
///   their `loop_step` must stay `@result && …` for error absorption;
/// - `map` / `filter` append with `@result + [elem]`, optionally guarded by
///   `cond ? … : @result`, and that shape must stay to append in place;
/// - anything else (`exists_one`) has its whole `loop_step` wrapped.
fn charge_iterations(comprehension: &mut ComprehensionExpr) {
    if !matches!(comprehension.loop_cond.expr, Expr::Literal(_)) {
        wrap(&mut comprehension.loop_cond);
        return;
    }
    let accu = comprehension.accu_var.clone();
    let is_accu = |e: &IdedExpr| matches!(&e.expr, Expr::Ident(name) if *name == accu);
    let step = &mut comprehension.loop_step;
    if let Expr::Call(call) = &mut step.expr
        && call.func_name == operators::CONDITIONAL
        && let [guard, append, otherwise] = call.args.as_mut_slice()
        && is_accu(otherwise)
        && appended_element(append, &is_accu).is_some()
    {
        wrap(guard);
        return;
    }
    if let Some(element) = appended_element(step, &is_accu) {
        wrap(element);
        return;
    }
    wrap(step);
}

fn appended_element<'e>(
    step: &'e mut IdedExpr,
    is_accu: &impl Fn(&IdedExpr) -> bool,
) -> Option<&'e mut IdedExpr> {
    let Expr::Call(call) = &mut step.expr else {
        return None;
    };
    if call.func_name != operators::ADD || call.target.is_some() {
        return None;
    }
    match call.args.as_mut_slice() {
        [lhs, rhs] if is_accu(lhs) => match &mut rhs.expr {
            Expr::List(list) if list.elements.len() == 1 => list.elements.first_mut(),
            _ => None,
        },
        _ => None,
    }
}

fn wrap(expr: &mut IdedExpr) {
    let id = expr.id;
    let inner = std::mem::take(expr);
    *expr = IdedExpr {
        id,
        expr: Expr::Call(CallExpr {
            func_name: CHARGE.to_owned(),
            target: None,
            args: vec![inner],
        }),
    };
}

#[derive(Debug, Clone)]
pub(super) struct Meter {
    spent: Arc<AtomicU64>,
    exceeded: Arc<AtomicBool>,
    max: u64,
}

impl Meter {
    pub(super) fn new(max: u64) -> Self {
        Self {
            spent: Arc::default(),
            exceeded: Arc::default(),
            max,
        }
    }

    pub(super) fn bind(&self, context: &mut Context<'_, '_>) {
        let meter = self.clone();
        context
            .add_function(
                CHARGE,
                move |Arguments(args): Arguments| -> Result<Value, ExecutionError> {
                    let value = args.first().cloned().unwrap_or(Value::Null);
                    meter.charge(&value)?;
                    Ok(value)
                },
            )
            .expect("the charge function's name cannot clash with a CEL overload");
    }

    pub(super) fn exceeded(&self) -> bool {
        self.exceeded.load(Ordering::Relaxed)
    }

    fn charge(&self, value: &Value) -> Result<(), ExecutionError> {
        let spent = self.spent.load(Ordering::Relaxed);
        let remaining = self.max.saturating_sub(spent);
        let cost = 1 + size(value, remaining);
        let spent = spent.saturating_add(cost);
        self.spent.store(spent, Ordering::Relaxed);
        if spent > self.max {
            self.exceeded.store(true, Ordering::Relaxed);
            return Err(ExecutionError::function_error(
                CHARGE,
                "evaluation cost limit exceeded",
            ));
        }
        Ok(())
    }
}

/// The size of `value` in cost units, counted up to `cap` only, so that
/// measuring a huge value costs no more than the budget it exhausts.
fn size(value: &Value, cap: u64) -> u64 {
    let mut total: u64 = 0;
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        let len = match value {
            Value::String(s) => s.len() as u64 / BYTES_PER_UNIT,
            Value::Bytes(b) => b.len() as u64 / BYTES_PER_UNIT,
            Value::List(items) => items.len() as u64,
            Value::Map(map) => map.map.len() as u64,
            _ => 0,
        };
        total = total.saturating_add(len);
        if total > cap {
            break;
        }
        match value {
            Value::List(items) => pending.extend(items.iter()),
            Value::Map(map) => pending.extend(map.map.values()),
            _ => {}
        }
    }
    total
}
