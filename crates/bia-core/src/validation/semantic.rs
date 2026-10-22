//! The semantic layer: checks that need the whole Definition, the Action Registry and the
//! resources the caller knows about.
//!
//! A Step may read `steps.<id>` only when Step `<id>` has finished on every path that reaches
//! it. Walking the Block tree in order, each Step list carries the set of Steps guaranteed to
//! have finished. A Step joins the set once it is passed. Steps nested in a Block join it after
//! the Block only when the Block cannot complete without them, which today means the branches
//! of a `flow.parallel` that joins `all`. Branch arms, loop bodies and `flow.try` bodies may run
//! partly or not at all. `on_error` starts with an empty set, because the Run may have failed
//! anywhere.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use super::params::{self, Resource};
use super::suggest::did_you_mean;
use super::{Code, Diagnostic, Severity, Validator, token};
use crate::action::Lookup;
use crate::dsl::{self, BLOCK_ACTIONS, Definition, Join, Params, Step, StepKind, TriggerKind};
use crate::expression::{CompileError, CompileErrorKind, Expression, References, Template};

pub(super) fn check(validator: &Validator<'_>, definition: &Definition) -> Vec<Diagnostic> {
    let mut checker = Checker {
        validator,
        definition,
        placed: Vec::new(),
        first: BTreeMap::new(),
        next: 0,
        diagnostics: Vec::new(),
    };
    checker.place(&definition.steps, "/steps", Section::Steps, &mut Vec::new());
    checker.place(
        &definition.on_error,
        "/on_error",
        Section::OnError,
        &mut Vec::new(),
    );
    checker.triggers();
    checker.walk(&definition.steps, Ctx::default(), &mut BTreeSet::new());
    checker.walk(&definition.on_error, Ctx::default(), &mut BTreeSet::new());
    checker.diagnostics
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Steps,
    OnError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Arm {
    Then,
    Else,
    Case(usize),
    Default,
    Branch(usize),
    Body,
    Catch,
}

impl Arm {
    fn path(self) -> String {
        match self {
            Self::Then => "then".into(),
            Self::Else => "else".into(),
            Self::Case(i) => format!("cases/{i}/steps"),
            Self::Default => "default".into(),
            Self::Branch(i) => format!("branches/{i}"),
            Self::Body => "body".into(),
            Self::Catch => "catch".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockKind {
    Branch,
    Switch,
    /// `joins_all`: the Block completes only once every branch has.
    Parallel {
        joins_all: bool,
    },
    Foreach,
    LoopUntil,
    Try,
}

impl BlockKind {
    fn of(kind: &StepKind) -> Option<Self> {
        Some(match kind {
            StepKind::Action { .. } => return None,
            StepKind::Branch(_) => Self::Branch,
            StepKind::Switch(_) => Self::Switch,
            StepKind::Parallel(parallel) => Self::Parallel {
                joins_all: matches!(parallel.join, None | Some(Join::All)),
            },
            StepKind::Foreach(_) => Self::Foreach,
            StepKind::LoopUntil(_) => Self::LoopUntil,
            StepKind::Try(_) => Self::Try,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Frame {
    block: usize,
    kind: BlockKind,
    arm: Arm,
}

#[derive(Debug)]
struct Placed<'a> {
    id: &'a str,
    path: String,
    section: Section,
    frames: Vec<Frame>,
}

fn arms(kind: &StepKind) -> Vec<(Arm, &[Step])> {
    match kind {
        StepKind::Action { .. } => Vec::new(),
        StepKind::Branch(branch) => vec![(Arm::Then, &branch.then), (Arm::Else, &branch.r#else)],
        StepKind::Switch(switch) => switch
            .cases
            .iter()
            .enumerate()
            .map(|(i, case)| (Arm::Case(i), case.steps.as_slice()))
            .chain([(Arm::Default, switch.default.as_slice())])
            .collect(),
        StepKind::Parallel(parallel) => parallel
            .branches
            .iter()
            .enumerate()
            .map(|(i, branch)| (Arm::Branch(i), branch.as_slice()))
            .collect(),
        StepKind::Foreach(foreach) => vec![(Arm::Body, &foreach.body)],
        StepKind::LoopUntil(loop_until) => vec![(Arm::Body, &loop_until.body)],
        StepKind::Try(r#try) => vec![(Arm::Body, &r#try.body), (Arm::Catch, &r#try.catch)],
    }
}

/// The variables in scope besides `input`, `steps`, `trigger` and `run`.
#[derive(Debug, Clone, Copy, Default)]
struct Ctx {
    item: bool,
    error: bool,
}

#[derive(Debug, Clone, Copy)]
struct Vantage {
    step: usize,
    /// The Expression runs after the Step's own body: a `flow.loop_until`'s `until`.
    after_body: bool,
}

struct Checker<'v, 'a> {
    validator: &'v Validator<'v>,
    definition: &'a Definition,
    placed: Vec<Placed<'a>>,
    first: BTreeMap<&'a str, usize>,
    next: usize,
    diagnostics: Vec<Diagnostic>,
}

impl<'a> Checker<'_, 'a> {
    fn push(
        &mut self,
        path: String,
        code: Code,
        severity: Severity,
        message: String,
        suggestion: Option<String>,
    ) {
        self.diagnostics.push(Diagnostic {
            path,
            code,
            severity,
            message,
            suggestion,
        });
    }

    fn error(&mut self, path: String, code: Code, message: String) {
        self.push(path, code, Severity::Error, message, None);
    }

    fn place(&mut self, steps: &'a [Step], path: &str, section: Section, frames: &mut Vec<Frame>) {
        for (i, step) in steps.iter().enumerate() {
            let path = format!("{path}/{i}");
            let order = self.placed.len();
            self.first.entry(step.id.as_str()).or_insert(order);
            self.placed.push(Placed {
                id: step.id.as_str(),
                path: path.clone(),
                section,
                frames: frames.clone(),
            });
            let Some(kind) = BlockKind::of(&step.kind) else {
                continue;
            };
            for (arm, nested) in arms(&step.kind) {
                frames.push(Frame {
                    block: order,
                    kind,
                    arm,
                });
                self.place(nested, &format!("{path}/{}", arm.path()), section, frames);
                frames.pop();
            }
        }
    }

    fn triggers(&mut self) {
        let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
        for (i, trigger) in self.definition.triggers.iter().enumerate() {
            let path = format!("/triggers/{i}");
            match seen.get(trigger.id.as_str()) {
                Some(first) => self.error(
                    format!("{path}/id"),
                    Code::DuplicateTriggerId,
                    format!(
                        "Trigger id '{}' is already used by /triggers/{first}",
                        trigger.id
                    ),
                ),
                None => {
                    seen.insert(trigger.id.as_str(), i);
                }
            }
            if let TriggerKind::Queue(queue) = &trigger.kind {
                self.resource(
                    Resource::Queue,
                    queue.queue.as_str(),
                    format!("{path}/queue"),
                );
            }
        }
    }

    /// Checks every Step of `steps`. `available` holds the Steps guaranteed to have finished
    /// before the first one, and on return also those guaranteed to have finished after the
    /// last one.
    fn walk(&mut self, steps: &'a [Step], ctx: Ctx, available: &mut BTreeSet<&'a str>) {
        for step in steps {
            let order = self.next;
            self.next += 1;
            let path = self.placed[order].path.clone();
            let at = Vantage {
                step: order,
                after_body: false,
            };
            self.placement(step, order, &path);
            if let Some(condition) = &step.r#if {
                self.condition(condition, format!("{path}/if"), at, ctx, available);
            }
            match &step.kind {
                StepKind::Action { action, params } => {
                    self.action(action.as_str(), params, &path, at, ctx, available);
                }
                StepKind::Branch(branch) => {
                    self.condition(&branch.when, format!("{path}/when"), at, ctx, available);
                    self.walk(&branch.then, ctx, &mut available.clone());
                    self.walk(&branch.r#else, ctx, &mut available.clone());
                }
                StepKind::Switch(switch) => {
                    self.condition(&switch.value, format!("{path}/value"), at, ctx, available);
                    for case in &switch.cases {
                        self.walk(&case.steps, ctx, &mut available.clone());
                    }
                    self.walk(&switch.default, ctx, &mut available.clone());
                }
                StepKind::Parallel(parallel) => {
                    let mut finished = BTreeSet::new();
                    for branch in &parallel.branches {
                        let mut seen = available.clone();
                        self.walk(branch, ctx, &mut seen);
                        finished.extend(seen);
                    }
                    if matches!(parallel.join, None | Some(Join::All)) {
                        available.extend(finished);
                    }
                }
                StepKind::Foreach(foreach) => {
                    self.condition(&foreach.items, format!("{path}/items"), at, ctx, available);
                    self.iterations(foreach.max_iterations, &path);
                    let body = Ctx { item: true, ..ctx };
                    self.walk(&foreach.body, body, &mut available.clone());
                }
                StepKind::LoopUntil(loop_until) => {
                    self.iterations(loop_until.max_iterations, &path);
                    let mut iteration = available.clone();
                    self.walk(&loop_until.body, ctx, &mut iteration);
                    let after_body = Vantage {
                        after_body: true,
                        ..at
                    };
                    self.condition(
                        &loop_until.until,
                        format!("{path}/until"),
                        after_body,
                        ctx,
                        &iteration,
                    );
                }
                StepKind::Try(r#try) => {
                    self.walk(&r#try.body, ctx, &mut available.clone());
                    let catch = Ctx { error: true, ..ctx };
                    self.walk(&r#try.catch, catch, &mut available.clone());
                }
            }
            available.insert(step.id.as_str());
        }
    }

    fn placement(&mut self, step: &Step, order: usize, path: &str) {
        let first = self.first[step.id.as_str()];
        if first != order {
            let message = format!(
                "Step id '{}' is already used by the Step at {}",
                step.id, self.placed[first].path
            );
            self.error(format!("{path}/id"), Code::DuplicateStepId, message);
        }
        let limits = self.validator.limits;
        if order == limits.max_steps {
            let message = format!(
                "The Definition has {} Steps, more than the limit of {}",
                self.placed.len(),
                limits.max_steps
            );
            self.error(path.to_owned(), Code::StepLimitExceeded, message);
        }
        // Reported once, at the outermost Block whose nested Steps are too deep.
        let depth = self.placed[order].frames.len();
        if depth == limits.max_nesting_depth && arms(&step.kind).iter().any(|(_, s)| !s.is_empty())
        {
            let message = format!(
                "Steps inside '{}' are nested {} Blocks deep, more than the limit of {}",
                step.id,
                depth + 1,
                limits.max_nesting_depth
            );
            self.error(path.to_owned(), Code::NestingLimitExceeded, message);
        }
    }

    fn iterations(&mut self, max_iterations: Option<u32>, path: &str) {
        let limit = self.validator.limits.max_iterations;
        if let Some(declared) = max_iterations
            && declared > limit
        {
            self.error(
                format!("{path}/max_iterations"),
                Code::IterationLimitExceeded,
                format!("max_iterations is {declared}, more than the limit of {limit}"),
            );
        }
    }

    fn action(
        &mut self,
        name: &str,
        params: &Params,
        path: &str,
        at: Vantage,
        ctx: Ctx,
        available: &BTreeSet<&'a str>,
    ) {
        let descriptor = match self.validator.actions.lookup(name) {
            Lookup::Enabled(action) => Some(action.descriptor().clone()),
            Lookup::Disabled(descriptor) => {
                self.push(
                    format!("{path}/action"),
                    Code::DisabledAction,
                    self.validator.target.publish_blocker(),
                    format!("Action '{name}' is disabled on this server"),
                    None,
                );
                Some(descriptor)
            }
            Lookup::Unknown => {
                let catalogue = self.validator.actions.catalogue();
                let known = catalogue
                    .iter()
                    .map(|entry| entry.descriptor.name.as_str())
                    .chain(BLOCK_ACTIONS);
                self.push(
                    format!("{path}/action"),
                    Code::UnknownAction,
                    Severity::Error,
                    format!("Action '{name}' does not exist"),
                    did_you_mean(name, known),
                );
                None
            }
        };
        for (key, value) in params {
            let path = format!("{path}/params/{}", token(key));
            self.param(value, path, at, ctx, available);
        }
        let Some(descriptor) = descriptor else {
            return;
        };
        for mismatch in params::mismatches(&descriptor, params) {
            self.error(
                format!("{path}/params{}", mismatch.location),
                Code::InvalidParams,
                format!(
                    "Params do not match the schema of '{name}': {}",
                    mismatch.message
                ),
            );
        }
        for reference in params::resources(&descriptor, params) {
            let path = format!("{path}/params{}", reference.location);
            self.resource(reference.resource, &reference.name, path);
        }
    }

    fn param(
        &mut self,
        value: &Value,
        path: String,
        at: Vantage,
        ctx: Ctx,
        available: &BTreeSet<&'a str>,
    ) {
        match value {
            Value::String(source) if source.contains("{{") => {
                match Template::parse_with(source, &self.validator.limits.expression) {
                    Ok(template) => {
                        self.references(template.references(), &path, at, ctx, available, true);
                    }
                    Err(error) => self.compile_error(error, path),
                }
            }
            Value::Array(items) => {
                for (i, item) in items.iter().enumerate() {
                    self.param(item, format!("{path}/{i}"), at, ctx, available);
                }
            }
            Value::Object(object) => {
                for (key, item) in object {
                    self.param(item, format!("{path}/{}", token(key)), at, ctx, available);
                }
            }
            _ => {}
        }
    }

    fn condition(
        &mut self,
        source: &dsl::Expression,
        path: String,
        at: Vantage,
        ctx: Ctx,
        available: &BTreeSet<&'a str>,
    ) {
        match Expression::compile_with(source.as_str(), &self.validator.limits.expression) {
            Ok(expression) => {
                self.references(expression.references(), &path, at, ctx, available, false);
            }
            Err(error) => self.compile_error(error, path),
        }
    }

    fn compile_error(&mut self, error: CompileError, path: String) {
        let code = match error.kind {
            CompileErrorKind::TooLong | CompileErrorKind::TooDeep => Code::ExpressionLimitExceeded,
            _ => Code::InvalidExpression,
        };
        self.error(path, code, error.to_string());
    }

    fn references(
        &mut self,
        references: &References,
        path: &str,
        at: Vantage,
        ctx: Ctx,
        available: &BTreeSet<&'a str>,
        in_params: bool,
    ) {
        if !in_params && (!references.secrets.is_empty() || references.dynamic_secrets) {
            let field = path.rsplit('/').next().unwrap_or_default();
            let read = if references.secrets.is_empty() {
                "secrets".to_owned()
            } else {
                references
                    .secrets
                    .iter()
                    .map(|name| format!("secrets.{name}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            self.push(
                path.to_owned(),
                Code::SecretOutsideParams,
                Severity::Error,
                format!("Secrets can only be read in params, but `{field}` reads {read}"),
                Some("Move the value into the params of the Step that needs it.".into()),
            );
        }
        for (variable, in_scope, inside) in [
            ("item", ctx.item, "a flow.foreach body"),
            ("index", ctx.item, "a flow.foreach body"),
            ("error", ctx.error, "the catch of a flow.try"),
        ] {
            if !in_scope && references.variables.contains(variable) {
                self.error(
                    path.to_owned(),
                    Code::VariableOutOfScope,
                    format!("'{variable}' is only in scope inside {inside}"),
                );
            }
        }
        for id in &references.steps {
            if !available.contains(id.as_str()) {
                self.unavailable(id, path, at);
            }
        }
    }

    /// Reports why Step `id` has not finished on every path to `at`.
    fn unavailable(&mut self, id: &str, path: &str, at: Vantage) {
        let Some(&target) = self.first.get(id) else {
            let suggestion = did_you_mean(id, self.first.keys().copied());
            self.push(
                path.to_owned(),
                Code::UnknownStepReference,
                Severity::Error,
                format!("No Step has the id '{id}'"),
                suggestion,
            );
            return;
        };
        let (code, message, suggestion) = self.explain(id, target, at);
        self.push(path.to_owned(), code, Severity::Error, message, suggestion);
    }

    fn explain(&self, id: &str, target: usize, at: Vantage) -> (Code, String, Option<String>) {
        let here = &self.placed[at.step];
        let there = &self.placed[target];
        let later = || {
            (
                Code::ForwardStepReference,
                format!("Step '{id}' runs later, so its Output is not available here"),
                None,
            )
        };
        let may_not_have_run = |message: String| (Code::StepMayNotHaveRun, message, None);
        if target == at.step {
            return (
                Code::ForwardStepReference,
                format!("Step '{id}' cannot read its own Output"),
                None,
            );
        }
        if here.frames.iter().any(|frame| frame.block == target) {
            return (
                Code::ForwardStepReference,
                format!("Step '{id}' has not finished here, because this Step runs inside it"),
                None,
            );
        }
        match (there.section, here.section) {
            (Section::Steps, Section::OnError) => {
                return may_not_have_run(format!(
                    "on_error runs after a failure at any point, so Step '{id}' may not have run"
                ));
            }
            (Section::OnError, Section::Steps) => {
                return may_not_have_run(format!(
                    "Step '{id}' is in on_error, which runs only after the Run fails"
                ));
            }
            _ => {}
        }
        let common = there
            .frames
            .iter()
            .zip(&here.frames)
            .take_while(|(a, b)| a == b)
            .count();
        if let (Some(theirs), Some(ours)) = (there.frames.get(common), here.frames.get(common))
            && theirs.block == ours.block
        {
            let block = self.placed[theirs.block].id;
            return match (theirs.kind, theirs.arm) {
                (BlockKind::Parallel { .. }, _) => may_not_have_run(format!(
                    "Step '{id}' is in another branch of '{block}', which runs concurrently"
                )),
                (BlockKind::Try, Arm::Body) => may_not_have_run(format!(
                    "Step '{id}' is in the body of '{block}', which may fail before it runs"
                )),
                (BlockKind::Try, _) => later(),
                _ => may_not_have_run(format!("Step '{id}' runs only on another arm of '{block}'")),
            };
        }
        // The outermost Block around the target that can complete without it.
        let Some(frame) = there.frames[common..].iter().find(|frame| {
            let joins_all = matches!(frame.kind, BlockKind::Parallel { joins_all: true });
            let own_body = at.after_body && frame.block == at.step;
            !joins_all && !own_body
        }) else {
            return later();
        };
        let in_own_body = at.after_body && there.frames.iter().any(|f| f.block == at.step);
        if frame.block >= at.step && !in_own_body {
            return later();
        }
        let block = self.placed[frame.block].id;
        match (frame.kind, frame.arm) {
            (BlockKind::Branch | BlockKind::Switch, _) => {
                may_not_have_run(format!("Step '{id}' runs only on one arm of '{block}'"))
            }
            (BlockKind::Parallel { .. }, _) => may_not_have_run(format!(
                "Step '{id}' is in a branch of '{block}', which can complete without it"
            )),
            (BlockKind::Foreach, _) => (
                Code::StepMayNotHaveRun,
                format!("Step '{id}' runs inside the body of '{block}', once per item"),
                Some(format!(
                    "Read the list of iteration Outputs, steps.{block}.output, instead."
                )),
            ),
            (BlockKind::LoopUntil, _) => may_not_have_run(format!(
                "Step '{id}' runs inside the body of '{block}', once per iteration"
            )),
            (BlockKind::Try, Arm::Body) => may_not_have_run(format!(
                "Step '{id}' is in the body of '{block}', which may fail before it runs"
            )),
            (BlockKind::Try, _) => may_not_have_run(format!(
                "Step '{id}' runs only when '{block}' catches an error"
            )),
        }
    }

    fn resource(&mut self, resource: Resource, name: &str, path: String) {
        let resources = self.validator.resources;
        let own_id = self.definition.id.as_str();
        let (exists, code, noun) = match resource {
            Resource::Queue => (resources.queue_exists(name), Code::UnknownQueue, "Queue"),
            Resource::Workflow => (
                name == own_id || resources.workflow_exists(name),
                Code::UnknownWorkflow,
                "Workflow",
            ),
        };
        if exists {
            return;
        }
        let known = match resource {
            Resource::Queue => resources.queue_names(),
            Resource::Workflow => {
                let mut ids = resources.workflow_ids();
                ids.push(own_id.to_owned());
                ids
            }
        };
        self.push(
            path,
            code,
            self.validator.target.publish_blocker(),
            format!("{noun} '{name}' does not exist"),
            did_you_mean(name, known.iter().map(String::as_str)),
        );
    }
}
