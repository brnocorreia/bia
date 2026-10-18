use std::collections::BTreeSet;

use cel::common::ast::{EntryExpr, Expr, IdedExpr, LiteralValue, operators};

/// The Step ids and Secret names an Expression or Template reads, found
/// statically, without evaluating anything.
///
/// `steps.a`, `steps['a']` and `steps.?a` all reference Step `a`. A read
/// whose id is not written literally, such as `steps[name]` or `size(steps)`,
/// can't be resolved and sets `dynamic_steps` instead (`dynamic_secrets` for
/// `secrets`). A comprehension variable named `steps` or `secrets` shadows the
/// scope inside its macro, as it does at run time.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct References {
    pub steps: BTreeSet<String>,
    pub secrets: BTreeSet<String>,
    pub dynamic_steps: bool,
    pub dynamic_secrets: bool,
}

#[derive(Clone, Copy)]
enum Root {
    Steps,
    Secrets,
}

impl References {
    pub(super) fn of(expr: &IdedExpr) -> Self {
        let mut references = Self::default();
        references.visit(expr, &mut Vec::new());
        references
    }

    pub(super) fn extend(&mut self, other: &Self) {
        self.steps.extend(other.steps.iter().cloned());
        self.secrets.extend(other.secrets.iter().cloned());
        self.dynamic_steps |= other.dynamic_steps;
        self.dynamic_secrets |= other.dynamic_secrets;
    }

    fn record(&mut self, root: Root, name: &str) {
        match root {
            Root::Steps => self.steps.insert(name.to_owned()),
            Root::Secrets => self.secrets.insert(name.to_owned()),
        };
    }

    fn record_dynamic(&mut self, root: Root) {
        match root {
            Root::Steps => self.dynamic_steps = true,
            Root::Secrets => self.dynamic_secrets = true,
        }
    }

    fn visit(&mut self, expr: &IdedExpr, bound: &mut Vec<String>) {
        match &expr.expr {
            Expr::Select(select) => match root(&select.operand, bound) {
                Some(root) => self.record(root, &select.field),
                None => self.visit(&select.operand, bound),
            },
            Expr::Call(call) => {
                let is_lookup = matches!(
                    call.func_name.as_str(),
                    operators::INDEX | operators::OPT_INDEX | operators::OPT_SELECT
                );
                if is_lookup
                    && call.target.is_none()
                    && let [operand, key] = call.args.as_slice()
                    && let Some(root) = root(operand, bound)
                {
                    match &key.expr {
                        Expr::Literal(LiteralValue::String(name)) => {
                            self.record(root, name.inner());
                        }
                        _ => {
                            self.record_dynamic(root);
                            self.visit(key, bound);
                        }
                    }
                    return;
                }
                if let Some(target) = &call.target {
                    self.visit(target, bound);
                }
                for arg in &call.args {
                    self.visit(arg, bound);
                }
            }
            Expr::Ident(_) => {
                if let Some(root) = root(expr, bound) {
                    self.record_dynamic(root);
                }
            }
            Expr::Comprehension(comprehension) => {
                self.visit(&comprehension.iter_range, bound);
                self.visit(&comprehension.accu_init, bound);
                let depth = bound.len();
                bound.push(comprehension.iter_var.clone());
                bound.extend(comprehension.iter_var2.iter().cloned());
                bound.push(comprehension.accu_var.clone());
                self.visit(&comprehension.loop_cond, bound);
                self.visit(&comprehension.loop_step, bound);
                self.visit(&comprehension.result, bound);
                bound.truncate(depth);
            }
            Expr::List(list) => {
                for element in &list.elements {
                    self.visit(element, bound);
                }
            }
            Expr::Map(map) => {
                for entry in &map.entries {
                    if let EntryExpr::MapEntry(entry) = &entry.expr {
                        self.visit(&entry.key, bound);
                        self.visit(&entry.value, bound);
                    }
                }
            }
            Expr::Struct(strct) => {
                for entry in &strct.entries {
                    if let EntryExpr::StructField(field) = &entry.expr {
                        self.visit(&field.value, bound);
                    }
                }
            }
            Expr::Literal(_) | Expr::Unspecified => {}
        }
    }
}

/// The scope root `expr` names, unless a comprehension variable shadows it.
fn root(expr: &IdedExpr, bound: &[String]) -> Option<Root> {
    let Expr::Ident(name) = &expr.expr else {
        return None;
    };
    if bound.iter().any(|b| b == name) {
        return None;
    }
    match name.as_str() {
        "steps" => Some(Root::Steps),
        "secrets" => Some(Root::Secrets),
        _ => None,
    }
}
