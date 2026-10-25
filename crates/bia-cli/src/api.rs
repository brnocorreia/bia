//! The subcommands that are clients of the HTTP API (ADR-0007, ADR-0047).
//!
//! Every command prints for people by default and as JSON with `--json`. Exit status is 0 on
//! success and 1 when the command failed, when validation found errors, or when a Dry Run
//! failed; usage mistakes exit 2.

use std::io::Read;
use std::path::PathBuf;

use anyhow::{Context, bail};
use bia_client::{
    Client, DryRunReport, Error, Run, RunQuery, RunStatus, RunSummary, Target, Version,
    VersionState,
};
use bia_core::validation::Diagnostic;
use chrono::{DateTime, Utc};
use clap::{Args, Subcommand, ValueEnum};
use serde::Serialize;
use serde_json::{Value, json};
use uuid::Uuid;

mod locate;
mod render;
mod source;

use source::Source;

pub(crate) struct Ctx {
    pub(crate) client: Client,
    pub(crate) url: String,
    pub(crate) json: bool,
}

#[derive(Debug, Subcommand)]
pub(crate) enum WorkflowCommand {
    /// Lists Workflows.
    List,
    /// Lists a Workflow's Versions.
    Versions { workflow: String },
    /// Prints a Version with its Definition as JSON.
    Get { workflow: String, version: i32 },
    /// Saves Definition files as Drafts: replaces the Workflow's latest Draft, or creates a
    /// new one. Directories are searched recursively for .json, .yaml and .yml files.
    Apply {
        /// Files or directories of Definitions.
        #[arg(short = 'f', long = "file", required = true, num_args = 1.., value_name = "PATH")]
        files: Vec<PathBuf>,
        /// Also publishes each Draft. Needs an `admin` Token.
        #[arg(long)]
        publish: bool,
        /// Also publishes and activates each Draft. Needs an `admin` Token.
        #[arg(long)]
        activate: bool,
    },
    /// Validates Definition files, or a stored Version, without changing anything.
    Validate {
        #[command(flatten)]
        subject: Subject,
        /// What to validate for: a Draft tolerates what Publishing refuses.
        #[arg(long, value_enum, default_value = "draft")]
        target: TargetArg,
    },
    /// Runs a Definition or a stored Version against Mocks, without storing anything. Exits 1
    /// when the Dry Run fails.
    DryRun {
        #[command(flatten)]
        subject: Subject,
        /// The Run input, a JSON or YAML file (`-` for standard input).
        #[arg(long, value_name = "FILE")]
        input: Option<String>,
        /// Mocks keyed by Step id, a JSON or YAML file (ADR-0046).
        #[arg(long, value_name = "FILE")]
        mocks: Option<String>,
    },
    /// Freezes a Draft as a Published Version. Needs an `admin` Token.
    Publish { workflow: String, version: i32 },
    /// Makes a Published Version the Active Version, which rolls back when it is older. Needs
    /// an `admin` Token.
    Activate { workflow: String, version: i32 },
    /// Prints the JSON Patch from one Version's Definition to another's.
    Diff {
        workflow: String,
        from: i32,
        to: i32,
    },
}

/// What a command works on: local Definition files, or a stored Version.
#[derive(Debug, Args)]
pub(crate) struct Subject {
    /// Files or directories of Definitions.
    #[arg(short = 'f', long = "file", num_args = 1.., value_name = "PATH", conflicts_with_all = ["workflow", "version"])]
    files: Vec<PathBuf>,
    /// The Workflow of a stored Version.
    workflow: Option<String>,
    /// The number of the stored Version.
    version: Option<i32>,
}

enum Resolved {
    Files(Vec<Source>),
    Stored(String, i32),
}

impl Subject {
    fn resolve(&self) -> anyhow::Result<Resolved> {
        match (&self.workflow, self.version, self.files.is_empty()) {
            (None, None, false) => Ok(Resolved::Files(source::collect(&self.files)?)),
            (Some(workflow), Some(version), true) => {
                Ok(Resolved::Stored(workflow.clone(), version))
            }
            _ => bail!("give either `-f <PATH>...` or `<WORKFLOW> <VERSION>`"),
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum TargetArg {
    Draft,
    Publish,
}

impl From<TargetArg> for Target {
    fn from(target: TargetArg) -> Self {
        match target {
            TargetArg::Draft => Self::Draft,
            TargetArg::Publish => Self::Publish,
        }
    }
}

#[derive(Debug, Subcommand)]
pub(crate) enum RunCommand {
    /// Starts a Run of a Workflow's Active Version from a manual Trigger.
    Trigger {
        workflow: String,
        /// The Run input, a JSON or YAML file (`-` for standard input).
        #[arg(long, value_name = "FILE")]
        input: Option<String>,
        /// Which manual Trigger, when the Version has several.
        #[arg(long)]
        trigger: Option<String>,
    },
    /// Shows a Run with its input and Timeline.
    Get { id: Uuid },
    /// Lists Runs, newest first.
    List {
        #[arg(long)]
        workflow: Option<String>,
        /// pending, running, sleeping, waiting, succeeded, failed or cancelled.
        #[arg(long, value_parser = parse_status)]
        status: Option<RunStatus>,
        /// Only Runs created at or after this RFC 3339 time.
        #[arg(long, value_name = "TIME")]
        created_from: Option<DateTime<Utc>>,
        /// Only Runs created before this RFC 3339 time.
        #[arg(long, value_name = "TIME")]
        created_to: Option<DateTime<Utc>>,
        /// Lists failed Runs with the entry that failed each (ignores `--status`).
        #[arg(long)]
        failed: bool,
        #[arg(long)]
        limit: Option<u32>,
        /// The `next_after` of the previous page.
        #[arg(long)]
        after: Option<Uuid>,
    },
    /// Requests cancellation of a Run.
    Cancel { id: Uuid },
    /// Starts a new Run with a finished Run's input and Version.
    Retry { id: Uuid },
    /// Resumes a failed Run from the Step that failed.
    Resume { id: Uuid },
}

#[derive(Debug, Subcommand)]
pub(crate) enum SecretCommand {
    /// Creates a Secret or replaces its value. The value is read from standard input, or from
    /// `--file`, never from an argument that `ps` would show; one trailing newline is dropped.
    /// Needs an `admin` Token.
    Set {
        /// The name Definitions read as `secrets.NAME`.
        name: String,
        /// Read the value from this file instead of standard input.
        #[arg(long, value_name = "FILE")]
        file: Option<PathBuf>,
    },
    /// Lists Secret names with their times. Values are never shown.
    List,
    /// Deletes a Secret. Needs an `admin` Token.
    Delete { name: String },
}

fn parse_status(text: &str) -> Result<RunStatus, String> {
    serde_json::from_value(Value::String(text.to_owned())).map_err(|_| {
        "expected pending, running, sleeping, waiting, succeeded, failed or cancelled".to_owned()
    })
}

pub(crate) async fn workflow(ctx: &Ctx, command: WorkflowCommand) -> bool {
    finish(ctx, workflow_command(ctx, command).await)
}

pub(crate) async fn run(ctx: &Ctx, command: RunCommand) -> bool {
    finish(ctx, run_command(ctx, command).await)
}

pub(crate) async fn secret(ctx: &Ctx, command: SecretCommand) -> bool {
    finish(ctx, secret_command(ctx, command).await)
}

async fn secret_command(ctx: &Ctx, command: SecretCommand) -> anyhow::Result<bool> {
    match command {
        SecretCommand::Set { name, file } => {
            let mut value = match file {
                Some(path) => std::fs::read_to_string(&path)
                    .with_context(|| format!("cannot read {}", path.display()))?,
                None => {
                    let mut value = String::new();
                    std::io::stdin()
                        .read_to_string(&mut value)
                        .context("cannot read the value from standard input")?;
                    value
                }
            };
            if value.ends_with('\n') {
                value.pop();
                if value.ends_with('\r') {
                    value.pop();
                }
            }
            if value.is_empty() {
                bail!("the value is empty; pipe it in or pass --file");
            }
            let secret = ctx.client.put_secret(&name, &value).await?;
            emit(ctx, &secret, || format!("set secret {}\n", secret.name))?;
        }
        SecretCommand::List => {
            let secrets = ctx.client.list_secrets().await?;
            emit(ctx, &json!({ "secrets": secrets }), || {
                let rows: Vec<_> = secrets
                    .iter()
                    .map(|s| {
                        vec![
                            s.name.clone(),
                            render::time(s.created_at),
                            render::time(s.updated_at),
                        ]
                    })
                    .collect();
                render::table(&["SECRET", "CREATED", "UPDATED"], &rows)
            })?;
        }
        SecretCommand::Delete { name } => {
            ctx.client.delete_secret(&name).await?;
            emit(ctx, &json!({ "deleted": name }), || {
                format!("deleted secret {name}\n")
            })?;
        }
    }
    Ok(true)
}

/// Prints a failure and turns the outcome into the process's success.
fn finish(ctx: &Ctx, outcome: anyhow::Result<bool>) -> bool {
    match outcome {
        Ok(success) => success,
        Err(error) => {
            report(ctx, &error);
            false
        }
    }
}

fn report(ctx: &Ctx, error: &anyhow::Error) {
    match error.downcast_ref::<Error>() {
        Some(Error::Api { status, body }) => print_error(ctx, status.as_u16(), body, None),
        Some(Error::Transport(error)) => print_failure(
            ctx,
            "unreachable",
            &format!("cannot reach the server at {}: {error}", ctx.url),
            &[],
            Some("set --url or BIA_URL to where `bia serve` listens"),
            None,
        ),
        _ => print_failure(ctx, "cli_error", &format!("{error:#}"), &[], None, None),
    }
}

fn print_error(ctx: &Ctx, status: u16, body: &bia_client::ErrorBody, source: Option<&Source>) {
    let hint = match status {
        401 => Some("set BIA_TOKEN, or `token` in the config file, to a valid Token"),
        403 => Some(
            "this Token's Scope does not allow the call; publishing and activation need an \
             `admin` Token",
        ),
        _ => None,
    };
    print_failure(
        ctx,
        &body.code,
        &body.message,
        &body.diagnostics,
        hint,
        source,
    );
}

fn print_failure(
    ctx: &Ctx,
    code: &str,
    message: &str,
    diagnostics: &[Diagnostic],
    hint: Option<&str>,
    source: Option<&Source>,
) {
    if ctx.json {
        let body =
            json!({ "error": { "code": code, "message": message, "diagnostics": diagnostics } });
        println!("{body:#}");
        return;
    }
    let mut text = format!("error[{code}]: {message}\n");
    render::diagnostics(&mut text, source, diagnostics);
    if let Some(hint) = hint {
        text.push_str(&format!("hint: {hint}\n"));
    }
    eprint!("{text}");
}

fn emit<T: Serialize>(ctx: &Ctx, value: &T, human: impl FnOnce() -> String) -> anyhow::Result<()> {
    if ctx.json {
        println!("{}", serde_json::to_string_pretty(value)?);
    } else {
        print!("{}", human());
    }
    Ok(())
}

async fn workflow_command(ctx: &Ctx, command: WorkflowCommand) -> anyhow::Result<bool> {
    let client = &ctx.client;
    match command {
        WorkflowCommand::List => {
            let workflows = client.list_workflows().await?;
            emit(ctx, &json!({ "workflows": workflows }), || {
                let rows: Vec<_> = workflows
                    .iter()
                    .map(|w| {
                        vec![
                            w.id.clone(),
                            w.active_version.map_or("-".to_owned(), |v| v.to_string()),
                            render::time(w.created_at),
                        ]
                    })
                    .collect();
                render::table(&["WORKFLOW", "ACTIVE", "CREATED"], &rows)
            })?;
        }
        WorkflowCommand::Versions { workflow } => {
            let versions = client.list_versions(&workflow).await?;
            emit(ctx, &json!({ "versions": versions }), || {
                let rows: Vec<_> = versions
                    .iter()
                    .map(|v| {
                        vec![
                            v.number.to_string(),
                            state(v.state).to_owned(),
                            if v.active { "active" } else { "" }.to_owned(),
                            render::time(v.created_at),
                        ]
                    })
                    .collect();
                render::table(&["VERSION", "STATE", "", "CREATED"], &rows)
            })?;
        }
        WorkflowCommand::Get { workflow, version } => {
            let version = client.get_version(&workflow, version).await?;
            println!("{}", serde_json::to_string_pretty(&version)?);
        }
        WorkflowCommand::Apply {
            files,
            publish,
            activate,
        } => return apply(ctx, &files, publish || activate, activate).await,
        WorkflowCommand::Validate { subject, target } => {
            return validate(ctx, &subject, target.into()).await;
        }
        WorkflowCommand::DryRun {
            subject,
            input,
            mocks,
        } => return dry_run(ctx, &subject, input, mocks).await,
        WorkflowCommand::Publish { workflow, version } => {
            let saved = client.publish_version(&workflow, version).await?;
            let version = &saved.version.summary;
            emit(ctx, &saved, || {
                let mut out = format!("published {} v{}\n", version.workflow_id, version.number);
                render::diagnostics(&mut out, None, &saved.diagnostics);
                out
            })?;
        }
        WorkflowCommand::Activate { workflow, version } => {
            let activated = client.activate_version(&workflow, version).await?;
            emit(ctx, &activated, || {
                format!("activated {} v{version}\n", activated.id)
            })?;
        }
        WorkflowCommand::Diff { workflow, from, to } => {
            let diff = client.diff_versions(&workflow, from, to).await?;
            emit(ctx, &diff, || {
                if diff.patch.is_empty() {
                    return format!("v{from} and v{to} have the same Definition\n");
                }
                let mut out = format!("{workflow} v{from} -> v{to}\n");
                for operation in &diff.patch {
                    out.push_str(&describe_operation(operation));
                }
                out
            })?;
        }
    }
    Ok(true)
}

fn state(state: VersionState) -> &'static str {
    match state {
        VersionState::Draft => "draft",
        VersionState::Published => "published",
    }
}

/// One JSON Patch operation as `+ /path value`, `- /path` or `~ /path value`.
fn describe_operation(operation: &Value) -> String {
    let text = |key: &str| operation.get(key).and_then(Value::as_str).unwrap_or("?");
    let value = operation
        .get("value")
        .map(|value| format!(" {value}"))
        .unwrap_or_default();
    match text("op") {
        "add" => format!("  + {}{value}\n", text("path")),
        "remove" => format!("  - {}\n", text("path")),
        "replace" => format!("  ~ {}{value}\n", text("path")),
        other => format!("  {other} {}{value}\n", text("path")),
    }
}

#[derive(Serialize)]
struct ApiFailure {
    code: String,
    message: String,
    diagnostics: Vec<Diagnostic>,
}

#[derive(Serialize)]
struct Applied {
    file: String,
    workflow: String,
    /// `created`, `replaced`, `unchanged` or `failed`.
    result: &'static str,
    version: Option<i32>,
    published: bool,
    activated: bool,
    diagnostics: Vec<Diagnostic>,
    error: Option<ApiFailure>,
}

async fn apply(
    ctx: &Ctx,
    paths: &[PathBuf],
    publish: bool,
    activate: bool,
) -> anyhow::Result<bool> {
    let sources = source::collect(paths)?;
    let mut applied = Vec::new();
    let mut text = String::new();
    for source in &sources {
        let outcome = apply_one(&ctx.client, source, publish, activate).await?;
        describe_applied(&mut text, source, &outcome);
        applied.push(outcome);
    }
    let success = applied
        .iter()
        .all(|a| a.result != "failed" && render::error_count(&a.diagnostics) == 0);
    emit(ctx, &json!({ "applied": applied }), || text)?;
    Ok(success)
}

fn describe_applied(out: &mut String, source: &Source, applied: &Applied) {
    let file = source.path.display();
    match (&applied.error, applied.version) {
        (Some(error), _) => {
            out.push_str(&format!(
                "failed {} ({file}): {}: {}\n",
                applied.workflow, error.code, error.message
            ));
            render::diagnostics(out, Some(source), &error.diagnostics);
        }
        (None, version) => {
            let mut line = format!(
                "{} {} v{} ({file})",
                applied.result,
                applied.workflow,
                version.unwrap_or_default()
            );
            if applied.published {
                line.push_str(", published");
            }
            if applied.activated {
                line.push_str(", activated");
            }
            out.push_str(&line);
            out.push('\n');
            render::diagnostics(out, Some(source), &applied.diagnostics);
        }
    }
}

/// Saves one file; a refusal by the API is part of the outcome, anything else aborts.
async fn apply_one(
    client: &Client,
    source: &Source,
    publish: bool,
    activate: bool,
) -> anyhow::Result<Applied> {
    let mut applied = Applied {
        file: source.path.display().to_string(),
        workflow: source.workflow_id().to_owned(),
        result: "failed",
        version: None,
        published: false,
        activated: false,
        diagnostics: Vec::new(),
        error: None,
    };
    match save(client, source, publish, activate, &mut applied).await {
        Ok(()) => Ok(applied),
        Err(Error::Api { body, .. }) => {
            applied.result = "failed";
            applied.error = Some(ApiFailure {
                code: body.code,
                message: body.message,
                diagnostics: body.diagnostics,
            });
            Ok(applied)
        }
        Err(error) => Err(error.into()),
    }
}

async fn save(
    client: &Client,
    source: &Source,
    publish: bool,
    activate: bool,
    applied: &mut Applied,
) -> Result<(), Error> {
    let id = source.workflow_id();
    let versions = match client.list_versions(id).await {
        Ok(versions) => versions,
        Err(Error::Api { body, .. }) if body.code == "workflow_not_found" => Vec::new(),
        Err(error) => return Err(error),
    };
    let latest = versions.iter().max_by_key(|version| version.number);
    let (number, already_published) = match latest {
        Some(latest) if latest.state == VersionState::Draft => {
            let saved = client
                .replace_draft(id, latest.number, &source.value)
                .await?;
            applied.result = "replaced";
            applied.diagnostics = saved.diagnostics;
            (latest.number, false)
        }
        Some(latest) if same_definition(client, id, latest.number, &source.value).await? => {
            applied.result = "unchanged";
            (latest.number, true)
        }
        _ => {
            let saved = client.create_draft(id, &source.value).await?;
            applied.result = "created";
            applied.diagnostics = saved.diagnostics;
            (saved.version.summary.number, false)
        }
    };
    applied.version = Some(number);
    if !publish || render::error_count(&applied.diagnostics) > 0 {
        return Ok(());
    }
    if !already_published {
        let published = client.publish_version(id, number).await?;
        applied.diagnostics = published.diagnostics;
        applied.published = true;
    }
    let is_active = versions
        .iter()
        .any(|version| version.number == number && version.active);
    if activate && !is_active {
        client.activate_version(id, number).await?;
        applied.activated = true;
    }
    Ok(())
}

/// Whether the Published Version `number` already holds `definition`, so applying the same
/// file twice does not pile up Versions.
async fn same_definition(
    client: &Client,
    workflow: &str,
    number: i32,
    definition: &Value,
) -> Result<bool, Error> {
    let version: Version = client.get_version(workflow, number).await?;
    Ok(&version.definition == definition)
}

#[derive(Serialize)]
struct Validated {
    /// The file, or `workflow` and `version` of a stored Version.
    #[serde(skip_serializing_if = "Option::is_none")]
    file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    workflow: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<i32>,
    valid: bool,
    diagnostics: Vec<Diagnostic>,
}

async fn validate(ctx: &Ctx, subject: &Subject, target: Target) -> anyhow::Result<bool> {
    let mut results = Vec::new();
    let mut sources = Vec::new();
    match subject.resolve()? {
        Resolved::Files(files) => {
            for source in files {
                let report = ctx.client.validate_definition(&source.value, target).await;
                let (valid, diagnostics) = match report {
                    Ok(report) => (report.valid, report.diagnostics),
                    // The body did not parse as a Definition; its Diagnostics say why.
                    Err(Error::Api { body, .. }) if body.code == "invalid_definition" => {
                        (false, body.diagnostics)
                    }
                    Err(error) => return Err(error.into()),
                };
                results.push(Validated {
                    file: Some(source.path.display().to_string()),
                    workflow: Some(source.workflow_id().to_owned()),
                    version: None,
                    valid,
                    diagnostics,
                });
                sources.push(Some(source));
            }
        }
        Resolved::Stored(workflow, version) => {
            let report = ctx
                .client
                .validate_version(&workflow, version, target)
                .await?;
            results.push(Validated {
                file: None,
                workflow: Some(workflow),
                version: Some(version),
                valid: report.valid,
                diagnostics: report.diagnostics,
            });
            sources.push(None);
        }
    }
    let success = results.iter().all(|result| result.valid);
    emit(ctx, &json!({ "results": results }), || {
        let mut out = String::new();
        for (result, source) in results.iter().zip(&sources) {
            render::diagnostics(&mut out, source.as_ref(), &result.diagnostics);
            let subject = match (&result.file, &result.workflow, result.version) {
                (Some(file), ..) => file.clone(),
                (None, Some(workflow), Some(version)) => format!("{workflow} v{version}"),
                _ => String::new(),
            };
            let errors = render::error_count(&result.diagnostics);
            if result.valid {
                out.push_str(&format!("{subject}: valid\n"));
            } else {
                out.push_str(&format!(
                    "{subject}: invalid, {}\n",
                    render::plural(errors, "error")
                ));
            }
        }
        out
    })?;
    Ok(success)
}

async fn dry_run(
    ctx: &Ctx,
    subject: &Subject,
    input: Option<String>,
    mocks: Option<String>,
) -> anyhow::Result<bool> {
    let input = input.map_or_else(|| Ok(json!({})), |path| source::document(&path))?;
    let mocks = mocks.map_or_else(|| Ok(json!({})), |path| source::document(&path))?;
    let (report, source): (DryRunReport, Option<Source>) = match subject.resolve()? {
        Resolved::Files(mut files) => {
            if files.len() != 1 {
                bail!(
                    "a Dry Run takes one Definition, but {} were given",
                    files.len()
                );
            }
            let source = files.remove(0);
            match ctx
                .client
                .dry_run_definition(&source.value, &input, &mocks)
                .await
            {
                Ok(report) => (report, Some(source)),
                // Point the refusal's Diagnostics at the file's lines.
                Err(Error::Api { status, body }) if !body.diagnostics.is_empty() => {
                    print_error(ctx, status.as_u16(), &body, Some(&source));
                    return Ok(false);
                }
                Err(error) => return Err(error.into()),
            }
        }
        Resolved::Stored(workflow, version) => (
            ctx.client
                .dry_run_version(&workflow, version, &input, &mocks)
                .await?,
            None,
        ),
    };
    let success = report.status == RunStatus::Succeeded;
    emit(ctx, &report, || {
        let mut out = format!("dry run {}\n", report.status.as_str());
        out.push_str(&render::timeline(&report.timeline));
        render::diagnostics(&mut out, source.as_ref(), &report.diagnostics);
        out
    })?;
    Ok(success)
}

async fn run_command(ctx: &Ctx, command: RunCommand) -> anyhow::Result<bool> {
    let client = &ctx.client;
    match command {
        RunCommand::Trigger {
            workflow,
            input,
            trigger,
        } => {
            let input = input.map_or_else(|| Ok(json!({})), |path| source::document(&path))?;
            let started = client
                .trigger_run(&workflow, &input, trigger.as_deref())
                .await?;
            emit(ctx, &started, || describe_started("started", &started))?;
        }
        RunCommand::Get { id } => {
            let run = client.get_run(id).await?;
            emit(ctx, &run, || describe_run(&run))?;
        }
        RunCommand::List {
            workflow,
            status,
            created_from,
            created_to,
            failed,
            limit,
            after,
        } => {
            let query = RunQuery {
                workflow_id: workflow,
                status,
                created_from,
                created_to,
                after,
                limit,
            };
            if failed {
                let page = client.list_failures(&query).await?;
                emit(ctx, &page, || {
                    let rows: Vec<_> = page
                        .runs
                        .iter()
                        .map(|run| {
                            let failure = run.failure.as_ref();
                            vec![
                                run.summary.id.to_string(),
                                run.summary.workflow_id.clone(),
                                format!("v{}", run.summary.version),
                                failure.and_then(|f| f.step_id.clone()).unwrap_or_default(),
                                failure
                                    .and_then(|f| f.error.as_ref())
                                    .and_then(|e| e.get("message"))
                                    .and_then(Value::as_str)
                                    .unwrap_or_default()
                                    .to_owned(),
                            ]
                        })
                        .collect();
                    let mut out =
                        render::table(&["RUN", "WORKFLOW", "VERSION", "STEP", "ERROR"], &rows);
                    next_page(&mut out, page.next_after);
                    out
                })?;
            } else {
                let page = client.list_runs(&query).await?;
                emit(ctx, &page, || {
                    let rows: Vec<_> = page
                        .runs
                        .iter()
                        .map(|run| {
                            vec![
                                run.id.to_string(),
                                run.workflow_id.clone(),
                                format!("v{}", run.version),
                                run.status.as_str().to_owned(),
                                render::time(run.created_at),
                            ]
                        })
                        .collect();
                    let mut out =
                        render::table(&["RUN", "WORKFLOW", "VERSION", "STATUS", "CREATED"], &rows);
                    next_page(&mut out, page.next_after);
                    out
                })?;
            }
        }
        RunCommand::Cancel { id } => {
            let run = client.cancel_run(id).await?;
            emit(ctx, &run, || describe_started("cancel requested for", &run))?;
        }
        RunCommand::Retry { id } => {
            let run = client.retry_run(id).await?;
            emit(ctx, &run, || describe_started("started", &run))?;
        }
        RunCommand::Resume { id } => {
            let run = client.resume_run(id).await?;
            emit(ctx, &run, || describe_started("resumed", &run))?;
        }
    }
    Ok(true)
}

fn next_page(out: &mut String, next_after: Option<Uuid>) {
    if let Some(after) = next_after {
        out.push_str(&format!("more: --after {after}\n"));
    }
}

fn describe_started(verb: &str, run: &RunSummary) -> String {
    format!(
        "{verb} run {} ({} v{}, {})\n",
        run.id,
        run.workflow_id,
        run.version,
        run.status.as_str()
    )
}

fn describe_run(run: &Run) -> String {
    let summary = &run.summary;
    let mut out = format!(
        "run       {}\nworkflow  {} v{}\nstatus    {}\ncreated   {}\n",
        summary.id,
        summary.workflow_id,
        summary.version,
        summary.status.as_str(),
        render::time(summary.created_at)
    );
    if let Some(finished) = summary.finished_at {
        out.push_str(&format!("finished  {}\n", render::time(finished)));
    }
    if let Some(of) = summary.retry_of {
        out.push_str(&format!("retry of  {of}\n"));
    }
    out.push_str(&format!("input     {}\n\n", run.input));
    out.push_str(&render::timeline(&run.timeline));
    out
}
