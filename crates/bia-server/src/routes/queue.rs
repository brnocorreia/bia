//! Queues: created, replaced and deleted by `admin`, read with their Messages by `admin` and
//! `agent` (ADR-0015, ADR-0059). Workflows publish to them with `queue.publish` and
//! `queue.publish_batch`.

use axum::extract::State;
use axum::http::StatusCode;
use bia_core::dsl::{
    DEFAULT_BATCH_SIZE, DEFAULT_MAX_RETRIES, Duration, MAX_BATCH_SIZE, QueueName,
    default_visibility_timeout,
};
use bia_store::StoreError;
use bia_store::queue::{
    Message, MessageFilter, MessageId, PgQueueRepository, Queue, QueueRepository, QueueSettings,
};
use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::{IntoParams, ToSchema};

use super::workflow::acquire;
use crate::auth::{Admin, Agent, AuthErrors};
use crate::{ApiError, ApiState, ErrorBody, ErrorCode, Json, Path, Query};

const DEFAULT_PAGE: u32 = 50;
const MAX_PAGE: u32 = 200;
/// Retries past this are a typo rather than a policy.
const MAX_RETRIES: u32 = 1_000;

/// A Queue's settings. Every field is optional and a field left out takes its default, so
/// putting a Queue again replaces all of its settings.
#[derive(Debug, Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct PutQueueRequest {
    /// How many more times a Message is delivered after its first delivery fails or is not
    /// acked in time, before it is dead-lettered. 0 to 1000; default 3.
    max_retries: Option<u32>,
    /// How long a delivered Message stays hidden from other consumers, a Duration such as
    /// `30s`. Default `30s`.
    #[schema(value_type = Option<String>, example = "30s")]
    visibility_timeout: Option<Duration>,
    /// How many Messages a consumer receives at once, 1 to 100. Default 10.
    batch_size: Option<u32>,
    /// The Queue that receives Messages which exhausted their retries. It must already exist.
    /// Without one, exhausted Messages stay in this Queue as dead letters.
    #[schema(value_type = Option<String>, example = "emails-dlq")]
    dead_letter: Option<QueueName>,
}

/// A Queue and its settings.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct QueueInfo {
    #[schema(example = "emails")]
    name: String,
    max_retries: u32,
    /// A Duration such as `30s`.
    #[schema(example = "30s")]
    visibility_timeout: String,
    batch_size: u32,
    dead_letter: Option<String>,
    created_at: DateTime<Utc>,
    /// When the settings were last put.
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct QueueList {
    /// Every Queue, ordered by name.
    queues: Vec<QueueInfo>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct QueueDeleted {
    name: String,
}

/// One Message of a Queue.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct MessageInfo {
    id: i64,
    queue: String,
    body: Value,
    published_at: DateTime<Utc>,
    /// When a consumer can receive it next: after its delay or retry backoff, or once the
    /// visibility timeout of the delivery holding it ends. Absent for a dead letter kept in a
    /// Queue without a Dead-Letter Queue, which is not delivered again.
    visible_at: Option<DateTime<Utc>>,
    /// Deliveries in this Queue so far.
    deliveries: i32,
    /// Whether a consumer holds it right now.
    in_flight: bool,
    /// Why the last delivery failed.
    last_error: Option<String>,
    /// When it exhausted its retries; absent unless it is a dead letter.
    dead_lettered_at: Option<DateTime<Utc>>,
    /// The Queue it exhausted its retries in.
    dead_lettered_from: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct MessageList {
    /// Oldest first.
    messages: Vec<MessageInfo>,
    /// Pass as `after` for the next page; absent on the last page.
    next_after: Option<i64>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
#[serde(deny_unknown_fields)]
pub(crate) struct MessageQuery {
    /// `true` lists only dead letters, `false` only the other Messages. Both by default.
    dead_letters: Option<bool>,
    /// Return the page after this Message id (a previous page's `next_after`).
    after: Option<i64>,
    /// Page size, 1 to 200. Defaults to 50.
    limit: Option<u32>,
}

/// Every Queue with its settings. Needs an `admin` or `agent` Token: an agent writing a
/// Definition needs the names to publish to.
#[utoipa::path(
    get,
    path = "/v1/queues",
    tag = "queues",
    security(("token" = [])),
    responses((status = OK, body = QueueList), AuthErrors)
)]
pub(crate) async fn list_queues(
    _: Agent,
    State(state): State<ApiState>,
) -> Result<Json<QueueList>, ApiError> {
    let mut conn = acquire(&state).await?;
    let queues = PgQueueRepository
        .list(&mut conn)
        .await
        .map_err(queue_error)?;
    Ok(Json(QueueList {
        queues: queues.into_iter().map(QueueInfo::from).collect(),
    }))
}

#[utoipa::path(
    get,
    path = "/v1/queues/{name}",
    tag = "queues",
    security(("token" = [])),
    params(("name" = String, Path, description = "The Queue's name.")),
    responses(
        (status = OK, body = QueueInfo),
        (status = NOT_FOUND, description = "`queue_not_found`.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn get_queue(
    _: Agent,
    State(state): State<ApiState>,
    Path(name): Path<String>,
) -> Result<Json<QueueInfo>, ApiError> {
    let name = queue_name(name)?;
    let mut conn = acquire(&state).await?;
    let queue = PgQueueRepository
        .get(&mut conn, &name)
        .await
        .map_err(queue_error)?
        .ok_or_else(|| queue_error(StoreError::QueueNotFound(name)))?;
    Ok(Json(queue.into()))
}

/// Creates the Queue, or replaces all of its settings. Needs an `admin` Token. Its Messages
/// are kept; new settings apply from their next delivery.
#[utoipa::path(
    put,
    path = "/v1/queues/{name}",
    tag = "queues",
    security(("token" = [])),
    params(("name" = String, Path, description = "Lowercase letters, digits, `_` or `-`, starting with a letter; at most 64 characters.")),
    request_body = PutQueueRequest,
    responses(
        (status = OK, body = QueueInfo),
        (status = BAD_REQUEST, description = "`malformed_request`: the name or a setting is not valid.", body = ErrorBody),
        (status = UNPROCESSABLE_ENTITY, description = "`dead_letter_queue_not_found`: `dead_letter` names no Queue.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn put_queue(
    _: Admin,
    State(state): State<ApiState>,
    Path(name): Path<String>,
    Json(request): Json<PutQueueRequest>,
) -> Result<Json<QueueInfo>, ApiError> {
    let name = queue_name(name)?;
    let settings = settings(&name, request)?;
    let mut conn = acquire(&state).await?;
    let queue = PgQueueRepository
        .put(&mut conn, &name, &settings, state.clock.now())
        .await
        .map_err(queue_error)?;
    tracing::info!(queue = %name, "queue put");
    Ok(Json(queue.into()))
}

/// Deletes a Queue and its Messages. Needs an `admin` Token. Steps that publish to it then
/// fail with `unknown_queue`, and Definitions naming it can no longer be published.
#[utoipa::path(
    delete,
    path = "/v1/queues/{name}",
    tag = "queues",
    security(("token" = [])),
    params(("name" = String, Path, description = "The Queue's name.")),
    responses(
        (status = OK, body = QueueDeleted),
        (status = NOT_FOUND, description = "`queue_not_found`.", body = ErrorBody),
        (status = CONFLICT, description = "`queue_in_use`: another Queue names it as its `dead_letter`.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn delete_queue(
    _: Admin,
    State(state): State<ApiState>,
    Path(name): Path<String>,
) -> Result<Json<QueueDeleted>, ApiError> {
    let name = queue_name(name)?;
    let mut conn = acquire(&state).await?;
    if !PgQueueRepository
        .delete(&mut conn, &name)
        .await
        .map_err(queue_error)?
    {
        return Err(queue_error(StoreError::QueueNotFound(name)));
    }
    tracing::info!(queue = %name, "queue deleted");
    Ok(Json(QueueDeleted {
        name: name.to_string(),
    }))
}

/// A Queue's Messages oldest first, dead letters included unless filtered out, paged with
/// `after` and `limit`. Needs an `admin` or `agent` Token.
#[utoipa::path(
    get,
    path = "/v1/queues/{name}/messages",
    tag = "queues",
    security(("token" = [])),
    params(("name" = String, Path, description = "The Queue's name."), MessageQuery),
    responses(
        (status = OK, body = MessageList),
        (status = BAD_REQUEST, description = "`malformed_request`.", body = ErrorBody),
        (status = NOT_FOUND, description = "`queue_not_found`.", body = ErrorBody),
        AuthErrors,
    )
)]
pub(crate) async fn list_messages(
    _: Agent,
    State(state): State<ApiState>,
    Path(name): Path<String>,
    Query(query): Query<MessageQuery>,
) -> Result<Json<MessageList>, ApiError> {
    let name = queue_name(name)?;
    let limit = query.limit.unwrap_or(DEFAULT_PAGE);
    if !(1..=MAX_PAGE).contains(&limit) {
        return Err(ApiError::malformed_request(format!(
            "`limit` is between 1 and {MAX_PAGE}"
        )));
    }
    let mut conn = acquire(&state).await?;
    let queues = PgQueueRepository;
    if queues
        .get(&mut conn, &name)
        .await
        .map_err(queue_error)?
        .is_none()
    {
        return Err(queue_error(StoreError::QueueNotFound(name)));
    }
    let filter = MessageFilter {
        queue: name,
        dead_letters: query.dead_letters,
        after: query.after.map(MessageId::new),
        // One more than asked for tells whether another page follows.
        limit: limit + 1,
    };
    let mut found = queues
        .list_messages(&mut conn, &filter)
        .await
        .map_err(queue_error)?;
    let size = limit as usize;
    let more = found.len() > size;
    found.truncate(size);
    let next_after = more
        .then(|| found.last().map(|message| message.id.get()))
        .flatten();
    let now = state.clock.now();
    Ok(Json(MessageList {
        messages: found
            .into_iter()
            .map(|message| message_info(message, now))
            .collect(),
        next_after,
    }))
}

fn settings(name: &QueueName, request: PutQueueRequest) -> Result<QueueSettings, ApiError> {
    let max_retries = request.max_retries.unwrap_or(DEFAULT_MAX_RETRIES);
    if max_retries > MAX_RETRIES {
        return Err(ApiError::malformed_request(format!(
            "`max_retries` is between 0 and {MAX_RETRIES}"
        )));
    }
    let batch_size = request.batch_size.unwrap_or(DEFAULT_BATCH_SIZE);
    if !(1..=MAX_BATCH_SIZE).contains(&batch_size) {
        return Err(ApiError::malformed_request(format!(
            "`batch_size` is between 1 and {MAX_BATCH_SIZE}"
        )));
    }
    let visibility = request
        .visibility_timeout
        .unwrap_or_else(default_visibility_timeout);
    let visibility_timeout = i64::try_from(visibility.as_millis())
        .ok()
        .and_then(TimeDelta::try_milliseconds)
        .filter(|timeout| *timeout > TimeDelta::zero())
        .ok_or_else(|| ApiError::malformed_request("`visibility_timeout` must be longer than 0"))?;
    if request.dead_letter.as_ref() == Some(name) {
        return Err(ApiError::malformed_request(
            "a Queue cannot be its own `dead_letter`",
        ));
    }
    Ok(QueueSettings {
        max_retries,
        visibility_timeout,
        batch_size,
        dead_letter: request.dead_letter,
    })
}

fn queue_name(name: String) -> Result<QueueName, ApiError> {
    QueueName::new(name).map_err(|e| ApiError::malformed_request(e.to_string()))
}

fn queue_error(error: StoreError) -> ApiError {
    let (status, code) = match &error {
        StoreError::QueueNotFound(_) => (StatusCode::NOT_FOUND, ErrorCode::QueueNotFound),
        StoreError::DeadLetterQueueNotFound(_) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::DeadLetterQueueNotFound,
        ),
        StoreError::QueueInUse(_) => (StatusCode::CONFLICT, ErrorCode::QueueInUse),
        _ => return ApiError::internal(&error),
    };
    // These variants name only Queues the caller sent.
    ApiError::new(status, code, error.to_string())
}

fn message_info(message: Message, now: DateTime<Utc>) -> MessageInfo {
    MessageInfo {
        id: message.id.get(),
        queue: message.queue.to_string(),
        body: message.body,
        published_at: message.published_at,
        visible_at: message.visible_at,
        deliveries: message.deliveries,
        in_flight: message.claimed_by.is_some() && message.visible_at.is_some_and(|at| at > now),
        last_error: message.last_error,
        dead_lettered_at: message.dead_lettered_at,
        dead_lettered_from: message.dead_lettered_from.map(|queue| queue.to_string()),
    }
}

impl From<Queue> for QueueInfo {
    fn from(queue: Queue) -> Self {
        let millis = u64::try_from(queue.settings.visibility_timeout.num_milliseconds());
        Self {
            name: queue.name.to_string(),
            max_retries: queue.settings.max_retries,
            visibility_timeout: Duration::from_millis(millis.unwrap_or(0)).to_string(),
            batch_size: queue.settings.batch_size,
            dead_letter: queue.settings.dead_letter.map(|queue| queue.to_string()),
            created_at: queue.created_at,
            updated_at: queue.updated_at,
        }
    }
}
