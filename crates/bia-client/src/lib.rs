//! Typed HTTP client for the bia API.
//!
//! Every method maps to one endpoint. A failed request comes back as [`Error::Api`] carrying
//! the server's error body, Diagnostics included. Every endpoint but the probes needs a Token,
//! set with [`Client::with_token`].

use std::fmt;

use bia_core::validation::Diagnostic;
use chrono::{DateTime, Utc};
use reqwest::{Method, StatusCode, Url};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("`{0}` is not a valid base URL")]
    BaseUrl(String),
    /// The server answered with its error body.
    #[error("{} ({}): {}", .body.code, .status, .body.message)]
    Api { status: StatusCode, body: ErrorBody },
    /// No usable answer: the server is unreachable, or answered with something that is not
    /// the API's.
    #[error(transparent)]
    Transport(#[from] reqwest::Error),
}

/// The body of every API error response.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ErrorBody {
    /// A stable snake_case code, e.g. `route_not_found` or `invalid_definition`.
    pub code: String,
    pub message: String,
    #[serde(default)]
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Status {
    pub status: String,
}

/// `GET /v1/schema`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SchemaDocument {
    pub definition: Value,
    pub queue: Value,
    pub actions: Vec<ActionEntry>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ActionEntry {
    pub name: String,
    pub category: String,
    pub description: String,
    /// `pure` or `side_effecting`.
    pub effect: String,
    pub dangerous: bool,
    pub enabled: bool,
    pub param_schema: Value,
    pub output_schema: Value,
    pub examples: Vec<ActionExample>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ActionExample {
    pub description: String,
    pub params: Value,
    pub output: Value,
}

/// What a Token may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Admin,
    Agent,
    Webhook,
}

/// The webhook Trigger a `webhook` Token is bound to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebhookBinding {
    pub workflow_id: String,
    pub trigger_id: String,
}

/// `POST /v1/tokens`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NewToken {
    pub name: String,
    pub scope: Scope,
    /// Required with [`Scope::Webhook`], refused otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub webhook: Option<WebhookBinding>,
}

/// A Token, without its value.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Token {
    pub id: Uuid,
    pub name: String,
    pub scope: Scope,
    pub webhook: Option<WebhookBinding>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

/// A Token just created, with the value the API shows only this once.
#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct CreatedToken {
    pub token: Token,
    pub value: String,
}

impl fmt::Debug for CreatedToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CreatedToken")
            .field("token", &self.token)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct TokenList {
    tokens: Vec<Token>,
}

/// A Workflow and the Version new Runs and Schedules use.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Workflow {
    pub id: String,
    pub active_version: Option<i32>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionState {
    Draft,
    Published,
}

/// A Version without its Definition.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct VersionSummary {
    pub workflow_id: String,
    pub number: i32,
    pub state: VersionState,
    /// Whether this is the Workflow's Active Version.
    pub active: bool,
    pub published_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// A Version with its Definition.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Version {
    #[serde(flatten)]
    pub summary: VersionSummary,
    pub definition: Value,
}

/// A Version just saved or published, with what validation found.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SavedVersion {
    pub version: Version,
    pub diagnostics: Vec<Diagnostic>,
}

/// What validation found; `valid` means no error Diagnostics.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ValidationReport {
    pub valid: bool,
    pub diagnostics: Vec<Diagnostic>,
}

/// What a Definition is validated for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Draft,
    Publish,
}

impl Target {
    fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Publish => "publish",
        }
    }
}

/// `GET /v1/workflows/{id}/diff`: an RFC 6902 JSON Patch from one Version's Definition to
/// the other's.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct VersionDiff {
    pub from: i32,
    pub to: i32,
    pub patch: Vec<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Pending,
    Running,
    Sleeping,
    Waiting,
    Succeeded,
    Failed,
    Cancelled,
}

impl RunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Sleeping => "sleeping",
            Self::Waiting => "waiting",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn is_finished(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}

/// How an Attempt, or the Run in an entry without a Step, ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptStatus {
    Succeeded,
    Failed,
    Skipped,
    Cancelled,
}

/// A Run without its input and Timeline.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RunSummary {
    pub id: Uuid,
    pub workflow_id: String,
    /// The Version the Run is pinned to.
    pub version: i32,
    pub status: RunStatus,
    pub trigger: Value,
    /// The Run this one retries, if `retry_run` started it.
    pub retry_of: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub cancel_requested_at: Option<DateTime<Utc>>,
}

/// One entry of a Run's Timeline (ADR-0033, ADR-0045).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimelineEntry {
    pub seq: i32,
    /// Absent for an entry about the Run as a whole.
    pub step_id: Option<String>,
    pub iteration_path: Option<String>,
    pub attempt: Option<i32>,
    pub action: Option<String>,
    pub status: AttemptStatus,
    pub input: Option<Value>,
    pub output: Option<Value>,
    /// `{ "class", "message", "details"?, "code"? }`.
    pub error: Option<Value>,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub duration_ms: i64,
}

/// The result of a Dry Run: the Timeline a real Run would have, not stored.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct DryRunReport {
    /// Always `true`.
    pub dry_run: bool,
    /// `succeeded` or `failed`.
    pub status: RunStatus,
    pub input: Value,
    /// The same entries a real Run's Timeline holds.
    pub timeline: Vec<TimelineEntry>,
    /// Findings that are not errors.
    pub diagnostics: Vec<Diagnostic>,
}

/// A Run with its input and Timeline.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Run {
    #[serde(flatten)]
    pub summary: RunSummary,
    pub input: Value,
    pub timeline: Vec<TimelineEntry>,
}

/// One page of Runs, newest first.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct RunPage {
    pub runs: Vec<RunSummary>,
    /// Pass as [`RunQuery::after`] for the next page; absent on the last.
    pub next_after: Option<Uuid>,
}

/// A failed Run and the entry that failed it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct FailedRun {
    #[serde(flatten)]
    pub summary: RunSummary,
    pub failure: Option<TimelineEntry>,
}

/// One page of failed Runs, newest first.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct FailurePage {
    pub runs: Vec<FailedRun>,
    pub next_after: Option<Uuid>,
}

/// Filters and paging of `list_runs` and `list_failures` (which ignores `status`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunQuery {
    pub workflow_id: Option<String>,
    pub status: Option<RunStatus>,
    /// Only Runs created at or after this time.
    pub created_from: Option<DateTime<Utc>>,
    /// Only Runs created before this time.
    pub created_to: Option<DateTime<Utc>>,
    pub after: Option<Uuid>,
    pub limit: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct WorkflowList {
    workflows: Vec<Workflow>,
}

#[derive(Debug, Deserialize)]
struct VersionList {
    versions: Vec<VersionSummary>,
}

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base: Url,
    token: Option<String>,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("base", &self.base.as_str())
            .finish_non_exhaustive()
    }
}

impl Client {
    /// `base_url` is where the API is served, e.g. `http://localhost:8080`.
    pub fn new(base_url: &str) -> Result<Self, Error> {
        let mut base = Url::parse(base_url).map_err(|_| Error::BaseUrl(base_url.to_owned()))?;
        if base.cannot_be_a_base() {
            return Err(Error::BaseUrl(base_url.to_owned()));
        }
        // Without it, joining a route would replace the base path's last segment.
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        Ok(Self {
            http: reqwest::Client::new(),
            base,
            token: None,
        })
    }

    /// Sends `token` as `Authorization: Bearer <token>` with every request.
    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self
    }

    pub async fn health(&self) -> Result<Status, Error> {
        self.send(Method::GET, "/health", None::<&()>).await
    }

    pub async fn ready(&self) -> Result<Status, Error> {
        self.send(Method::GET, "/ready", None::<&()>).await
    }

    pub async fn schema(&self) -> Result<SchemaDocument, Error> {
        self.send(Method::GET, "/v1/schema", None::<&()>).await
    }

    pub async fn create_token(&self, token: &NewToken) -> Result<CreatedToken, Error> {
        self.send(Method::POST, "/v1/tokens", Some(token)).await
    }

    pub async fn list_tokens(&self) -> Result<Vec<Token>, Error> {
        let list: TokenList = self.send(Method::GET, "/v1/tokens", None::<&()>).await?;
        Ok(list.tokens)
    }

    pub async fn revoke_token(&self, id: Uuid) -> Result<Token, Error> {
        self.send(
            Method::POST,
            &format!("/v1/tokens/{id}/revoke"),
            None::<&()>,
        )
        .await
    }

    /// Mints the first admin Token of a fresh install with the server's bootstrap key, which
    /// is sent instead of this client's Token.
    pub async fn bootstrap_token(
        &self,
        bootstrap_key: &str,
        name: &str,
    ) -> Result<CreatedToken, Error> {
        #[derive(Serialize)]
        struct Request<'a> {
            name: &'a str,
        }
        let client = self.clone().with_token(bootstrap_key);
        client
            .send(
                Method::POST,
                "/v1/tokens/bootstrap",
                Some(&Request { name }),
            )
            .await
    }

    pub async fn list_workflows(&self) -> Result<Vec<Workflow>, Error> {
        let list: WorkflowList = self.send(Method::GET, "/v1/workflows", None::<&()>).await?;
        Ok(list.workflows)
    }

    pub async fn get_workflow(&self, id: &str) -> Result<Workflow, Error> {
        self.send(Method::GET, &format!("/v1/workflows/{id}"), None::<&()>)
            .await
    }

    pub async fn list_versions(&self, workflow: &str) -> Result<Vec<VersionSummary>, Error> {
        let list: VersionList = self
            .send(
                Method::GET,
                &format!("/v1/workflows/{workflow}/versions"),
                None::<&()>,
            )
            .await?;
        Ok(list.versions)
    }

    pub async fn get_version(&self, workflow: &str, number: i32) -> Result<Version, Error> {
        self.send(
            Method::GET,
            &format!("/v1/workflows/{workflow}/versions/{number}"),
            None::<&()>,
        )
        .await
    }

    /// Saves `definition`, a Definition as JSON, as a new Draft of `workflow`, creating the
    /// Workflow if it is new.
    pub async fn create_draft(
        &self,
        workflow: &str,
        definition: &Value,
    ) -> Result<SavedVersion, Error> {
        self.send(
            Method::POST,
            &format!("/v1/workflows/{workflow}/versions"),
            Some(definition),
        )
        .await
    }

    pub async fn replace_draft(
        &self,
        workflow: &str,
        number: i32,
        definition: &Value,
    ) -> Result<SavedVersion, Error> {
        self.send(
            Method::PUT,
            &format!("/v1/workflows/{workflow}/versions/{number}"),
            Some(definition),
        )
        .await
    }

    pub async fn validate_version(
        &self,
        workflow: &str,
        number: i32,
        target: Target,
    ) -> Result<ValidationReport, Error> {
        let path = format!(
            "/v1/workflows/{workflow}/versions/{number}/validate?target={}",
            target.as_str()
        );
        self.send(Method::POST, &path, None::<&()>).await
    }

    /// Validates a Definition that need not be stored.
    pub async fn validate_definition(
        &self,
        definition: &Value,
        target: Target,
    ) -> Result<ValidationReport, Error> {
        let path = format!("/v1/validate?target={}", target.as_str());
        self.send(Method::POST, &path, Some(definition)).await
    }

    /// Needs an `admin` Token.
    pub async fn publish_version(
        &self,
        workflow: &str,
        number: i32,
    ) -> Result<SavedVersion, Error> {
        self.send(
            Method::POST,
            &format!("/v1/workflows/{workflow}/versions/{number}/publish"),
            None::<&()>,
        )
        .await
    }

    /// Makes a Published Version the Active Version; an older one is a rollback. Needs an
    /// `admin` Token.
    pub async fn activate_version(&self, workflow: &str, number: i32) -> Result<Workflow, Error> {
        self.send(
            Method::POST,
            &format!("/v1/workflows/{workflow}/versions/{number}/activate"),
            None::<&()>,
        )
        .await
    }

    pub async fn diff_versions(
        &self,
        workflow: &str,
        from: i32,
        to: i32,
    ) -> Result<VersionDiff, Error> {
        self.send(
            Method::GET,
            &format!("/v1/workflows/{workflow}/diff?from={from}&to={to}"),
            None::<&()>,
        )
        .await
    }

    /// Starts a Run of the Workflow's Active Version from a manual Trigger. `trigger_id`
    /// picks one when the Version has several.
    pub async fn trigger_run(
        &self,
        workflow: &str,
        input: &Value,
        trigger_id: Option<&str>,
    ) -> Result<RunSummary, Error> {
        let mut body = serde_json::json!({ "input": input });
        if let Some(trigger_id) = trigger_id {
            body["trigger_id"] = trigger_id.into();
        }
        self.send(
            Method::POST,
            &format!("/v1/workflows/{workflow}/runs"),
            Some(&body),
        )
        .await
    }

    pub async fn list_runs(&self, query: &RunQuery) -> Result<RunPage, Error> {
        let path = Self::with_query("/v1/runs", query, true)?;
        self.send(Method::GET, &path, None::<&()>).await
    }

    /// Failed Runs, each with the entry that failed it.
    pub async fn list_failures(&self, query: &RunQuery) -> Result<FailurePage, Error> {
        let path = Self::with_query("/v1/runs/failures", query, false)?;
        self.send(Method::GET, &path, None::<&()>).await
    }

    pub async fn get_run(&self, id: Uuid) -> Result<Run, Error> {
        self.send(Method::GET, &format!("/v1/runs/{id}"), None::<&()>)
            .await
    }

    /// Requests cancellation; the Run ends `cancelled` a moment later.
    pub async fn cancel_run(&self, id: Uuid) -> Result<RunSummary, Error> {
        self.send(Method::POST, &format!("/v1/runs/{id}/cancel"), None::<&()>)
            .await
    }

    /// Resumes a failed Run from the Step that failed, on its own Version.
    pub async fn resume_run(&self, id: Uuid) -> Result<RunSummary, Error> {
        self.send(Method::POST, &format!("/v1/runs/{id}/resume"), None::<&()>)
            .await
    }

    /// Starts a new Run with a finished Run's input and Version.
    pub async fn retry_run(&self, id: Uuid) -> Result<RunSummary, Error> {
        self.send(Method::POST, &format!("/v1/runs/{id}/retry"), None::<&()>)
            .await
    }

    /// Dry-runs a stored Version. `mocks` maps Step ids to Mocks, e.g.
    /// `{ "fetch": { "output": { "status": 200 } } }`; see ADR-0046 for the format.
    pub async fn dry_run_version(
        &self,
        workflow: &str,
        number: i32,
        input: &Value,
        mocks: &Value,
    ) -> Result<DryRunReport, Error> {
        self.send(
            Method::POST,
            &format!("/v1/workflows/{workflow}/versions/{number}/dry-run"),
            Some(&serde_json::json!({ "input": input, "mocks": mocks })),
        )
        .await
    }

    /// Dry-runs a Definition that need not be stored.
    pub async fn dry_run_definition(
        &self,
        definition: &Value,
        input: &Value,
        mocks: &Value,
    ) -> Result<DryRunReport, Error> {
        self.send(
            Method::POST,
            "/v1/dry-run",
            Some(&serde_json::json!({
                "definition": definition,
                "input": input,
                "mocks": mocks,
            })),
        )
        .await
    }

    fn with_query(path: &str, query: &RunQuery, status: bool) -> Result<String, Error> {
        let mut pairs: Vec<(&str, String)> = Vec::new();
        if let Some(workflow) = &query.workflow_id {
            pairs.push(("workflow_id", workflow.clone()));
        }
        if let Some(run_status) = query.status.filter(|_| status) {
            pairs.push(("status", run_status.as_str().to_owned()));
        }
        if let Some(from) = query.created_from {
            pairs.push(("created_from", from.to_rfc3339()));
        }
        if let Some(to) = query.created_to {
            pairs.push(("created_to", to.to_rfc3339()));
        }
        if let Some(after) = query.after {
            pairs.push(("after", after.to_string()));
        }
        if let Some(limit) = query.limit {
            pairs.push(("limit", limit.to_string()));
        }
        // Only for its query encoding: `send` joins the path onto the real base.
        let url = Url::parse_with_params(&format!("http://localhost{path}"), pairs)
            .map_err(|_| Error::BaseUrl(path.to_owned()))?;
        Ok(match url.query() {
            Some(query) => format!("{}?{query}", url.path()),
            None => url.path().to_owned(),
        })
    }

    async fn send<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<&impl Serialize>,
    ) -> Result<T, Error> {
        let url = self
            .base
            .join(path.trim_start_matches('/'))
            .map_err(|_| Error::BaseUrl(self.base.to_string()))?;
        let mut request = self.http.request(method, url);
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await?;
        let status = response.status();
        if status.is_success() {
            return Ok(response.json().await?);
        }
        Err(Error::Api {
            status,
            body: response.json().await?,
        })
    }
}
