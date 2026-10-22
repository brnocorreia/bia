//! `http.request`: sends one HTTP request and returns the response.

use std::collections::BTreeMap;
use std::error::Error as _;

use base64::Engine as _;
use bia_core::action::{
    Action, ActionCall, ActionDescriptor, ActionError, ActionExample, BoxFuture, CancellationToken,
    Effect, IdempotencyKey, Output,
};
use bia_core::dsl::{ActionName, Duration};
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use reqwest::{Client, Url, redirect};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::BuiltinError;
use crate::params::{ParamReader, object, schema_for};

/// How long a request may take when its Params set no `timeout`.
const DEFAULT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// The most redirects followed before the request fails.
const MAX_REDIRECTS: usize = 10;

/// The largest response body read. A bigger one fails the Attempt, since it would exceed any
/// sensible Output size anyway.
const MAX_RESPONSE_BYTES: usize = 10 * 1024 * 1024;

const IDEMPOTENCY_KEY: &str = "idempotency-key";

/// The Params of `http.request`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(
    title = "http.request params",
    extend("not" = { "required": ["json", "text"] })
)]
struct Params {
    /// The absolute `http` or `https` URL to call. Query parameters already in it are kept.
    url: String,
    /// The HTTP method.
    #[serde(default)]
    method: Method,
    /// Request headers. `Idempotency-Key` is set to the Step's Idempotency Key unless given
    /// here.
    #[serde(default)]
    headers: BTreeMap<String, String>,
    /// Query parameters appended to the URL.
    #[serde(default)]
    query: BTreeMap<String, QueryValue>,
    /// A JSON request body. Sets `Content-Type: application/json` unless a header sets it.
    /// Cannot be combined with `text`.
    json: Option<Value>,
    /// A text request body. Sets `Content-Type: text/plain; charset=utf-8` unless a header
    /// sets it. Cannot be combined with `json`.
    text: Option<String>,
    /// Credentials sent in the `Authorization` header.
    auth: Option<Auth>,
    /// How long the whole request may take, response body included. Defaults to `30s`.
    timeout: Option<Duration>,
    /// Statuses outside 2xx that also count as success, e.g. `[404]` to read a missing
    /// resource as an Output instead of failing.
    #[serde(default)]
    success_statuses: Vec<StatusCode>,
}

#[derive(Debug, Default, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "UPPERCASE")]
enum Method {
    #[default]
    Get,
    Head,
    Post,
    Put,
    Patch,
    Delete,
    Options,
}

impl From<Method> for reqwest::Method {
    fn from(method: Method) -> Self {
        match method {
            Method::Get => Self::GET,
            Method::Head => Self::HEAD,
            Method::Post => Self::POST,
            Method::Put => Self::PUT,
            Method::Patch => Self::PATCH,
            Method::Delete => Self::DELETE,
            Method::Options => Self::OPTIONS,
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(untagged)]
enum QueryValue {
    String(String),
    Number(serde_json::Number),
    Bool(bool),
}

impl QueryValue {
    fn to_query_string(&self) -> String {
        match self {
            Self::String(value) => value.clone(),
            Self::Number(value) => value.to_string(),
            Self::Bool(value) => value.to_string(),
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Auth {
    /// HTTP Basic authentication.
    Basic { username: String, password: String },
    /// A bearer token, usually from `secrets.NAME`.
    Bearer { token: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(transparent)]
struct StatusCode(#[schemars(range(min = 100, max = 599))] u16);

/// The Output of `http.request`, and the `details` of an error for an unsuccessful status.
#[derive(Debug, Serialize, JsonSchema)]
#[schemars(title = "http.request output")]
struct Response {
    /// The response status code.
    status: u16,
    /// Response headers by lowercase name. Repeated headers are joined with `, `.
    headers: BTreeMap<String, String>,
    /// The response body: parsed JSON when `body_encoding` is `json`, otherwise a string.
    body: Value,
    body_encoding: BodyEncoding,
}

/// How the response body is represented in `body`.
#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum BodyEncoding {
    /// The response declared a JSON content type and parsed as JSON.
    Json,
    /// Any other UTF-8 body, as a string.
    Text,
    /// A body that is not UTF-8, as a base64 string.
    Base64,
}

/// Sends one HTTP request and returns its status, headers and body.
///
/// A 2xx status, or one listed in `success_statuses`, succeeds. 5xx, 408 and 429 statuses,
/// network errors and timeouts are `retryable`; every other status and invalid Params are
/// `fatal` (ADR-0012).
#[derive(Debug)]
pub struct Request {
    descriptor: ActionDescriptor,
    params: ParamReader,
    client: Client,
}

impl Request {
    /// The client verifies TLS certificates against the system trust store and follows up to
    /// ten redirects, dropping credentials when one leads to another host.
    pub fn new() -> Result<Self, BuiltinError> {
        let client = Client::builder()
            .redirect(redirect::Policy::limited(MAX_REDIRECTS))
            .build()
            .map_err(BuiltinError::HttpClient)?;
        let descriptor = descriptor();
        let params = ParamReader::new(&descriptor);
        Ok(Self {
            descriptor,
            params,
            client,
        })
    }

    async fn send(&self, call: ActionCall) -> Result<Output, ActionError> {
        let params: Params = self.params.read(&call.params)?;
        let (request, success_statuses) = self.build(params, &call.idempotency_key)?;
        let response = request.send().await.map_err(transport_error)?;
        let response = read(response).await?;
        let status = response.status;
        let output = serde_json::to_value(&response)
            .map_err(|_| ActionError::fatal("the response cannot be represented as JSON"))?;
        if (200..300).contains(&status) || success_statuses.contains(&StatusCode(status)) {
            Ok(output)
        } else {
            Err(status_error(status).with_details(output))
        }
    }

    fn build(
        &self,
        params: Params,
        key: &IdempotencyKey,
    ) -> Result<(reqwest::RequestBuilder, Vec<StatusCode>), ActionError> {
        // Messages name the param but never echo its value, which may hold a Secret.
        let mut url = Url::parse(&params.url)
            .map_err(|_| ActionError::fatal("`url` is not a valid absolute URL"))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(ActionError::fatal(
                "`url` must use the http or https scheme",
            ));
        }
        if !params.query.is_empty() {
            let mut pairs = url.query_pairs_mut();
            for (name, value) in &params.query {
                pairs.append_pair(name, &value.to_query_string());
            }
        }

        let mut headers = HeaderMap::new();
        for (name, value) in &params.headers {
            let name = HeaderName::try_from(name.as_str()).map_err(|_| {
                ActionError::fatal(format!("`headers`: `{name}` is not a valid header name"))
            })?;
            let mut value = HeaderValue::try_from(value.as_str()).map_err(|_| {
                ActionError::fatal(format!("`headers`: the value of `{name}` is not valid"))
            })?;
            value.set_sensitive(true);
            headers.append(name, value);
        }
        if !headers.contains_key(IDEMPOTENCY_KEY) {
            let value = HeaderValue::try_from(key.as_str())
                .map_err(|_| ActionError::fatal("the Idempotency Key is not a valid header"))?;
            headers.insert(IDEMPOTENCY_KEY, value);
        }

        let mut request = self
            .client
            .request(params.method.into(), url)
            .timeout(params.timeout.map_or(DEFAULT_TIMEOUT, Duration::to_std));
        request = match (params.json, params.text) {
            (Some(_), Some(_)) => {
                return Err(ActionError::fatal(
                    "invalid Params: `json` and `text` cannot both be set",
                ));
            }
            (Some(json), None) => {
                let body = serde_json::to_vec(&json)
                    .map_err(|_| ActionError::fatal("`json` cannot be serialised"))?;
                default_content_type(&mut headers, "application/json");
                request.body(body)
            }
            (None, Some(text)) => {
                default_content_type(&mut headers, "text/plain; charset=utf-8");
                request.body(text)
            }
            (None, None) => request,
        };
        request = match params.auth {
            Some(Auth::Basic { username, password }) => {
                request.basic_auth(username, Some(password))
            }
            Some(Auth::Bearer { token }) => request.bearer_auth(token),
            None => request,
        };
        Ok((request.headers(headers), params.success_statuses))
    }
}

impl Action for Request {
    fn descriptor(&self) -> &ActionDescriptor {
        &self.descriptor
    }

    fn execute(
        &self,
        call: ActionCall,
        cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<Output, ActionError>> {
        Box::pin(async move {
            // Dropping the request future closes its connection, aborting the request.
            tokio::select! {
                result = self.send(call) => result,
                () = cancel.cancelled() => Err(ActionError::retryable("the request was cancelled")),
            }
        })
    }
}

fn default_content_type(headers: &mut HeaderMap, value: &'static str) {
    headers
        .entry(CONTENT_TYPE)
        .or_insert(HeaderValue::from_static(value));
}

async fn read(mut response: reqwest::Response) -> Result<Response, ActionError> {
    let status = response.status().as_u16();
    let mut headers = BTreeMap::<String, String>::new();
    for (name, value) in response.headers() {
        let value = String::from_utf8_lossy(value.as_bytes());
        headers
            .entry(name.as_str().to_owned())
            .and_modify(|joined| {
                joined.push_str(", ");
                joined.push_str(&value);
            })
            .or_insert_with(|| value.into_owned());
    }
    let is_json = headers
        .get(CONTENT_TYPE.as_str())
        .is_some_and(|value| is_json_media_type(value));

    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(transport_error)? {
        if bytes.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return Err(ActionError::fatal(format!(
                "the response body is larger than {MAX_RESPONSE_BYTES} bytes"
            ))
            .with_details(json!({ "status": status })));
        }
        bytes.extend_from_slice(&chunk);
    }

    let (body, body_encoding) = match String::from_utf8(bytes) {
        Ok(text) => match is_json.then(|| serde_json::from_str::<Value>(&text)) {
            Some(Ok(value)) => (value, BodyEncoding::Json),
            // A JSON content type with a body that does not parse, e.g. an empty 204, is
            // kept as text rather than failing a request that succeeded.
            _ => (Value::String(text), BodyEncoding::Text),
        },
        Err(error) => (
            Value::String(base64::engine::general_purpose::STANDARD.encode(error.into_bytes())),
            BodyEncoding::Base64,
        ),
    };
    Ok(Response {
        status,
        headers,
        body,
        body_encoding,
    })
}

/// `application/json`, or any `+json` type such as `application/problem+json`.
fn is_json_media_type(content_type: &str) -> bool {
    let essence = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    essence == "application/json" || essence.ends_with("+json")
}

fn status_error(status: u16) -> ActionError {
    let message = format!("the server responded with status {status}");
    // 408 and 429 are the server asking to be tried again later.
    if (500..600).contains(&status) || matches!(status, 408 | 429) {
        ActionError::retryable(message)
    } else {
        ActionError::fatal(message)
    }
}

/// Network errors and timeouts may pass on their own. A request that could not even be built,
/// or that redirected too often, will fail the same way every time.
fn transport_error(error: reqwest::Error) -> ActionError {
    // The URL may carry a Secret in its query, so it is left out of the message.
    let error = error.without_url();
    let fatal = error.is_builder() || error.is_redirect();
    let kind = if error.is_timeout() {
        "the request timed out"
    } else if error.is_connect() {
        "cannot connect to the server"
    } else if error.is_redirect() {
        "too many redirects"
    } else {
        "the request failed"
    };
    let mut message = kind.to_owned();
    let mut source = error.source();
    while let Some(cause) = source {
        message.push_str(": ");
        message.push_str(&cause.to_string());
        source = cause.source();
    }
    if fatal {
        ActionError::fatal(message)
    } else {
        ActionError::retryable(message)
    }
}

fn descriptor() -> ActionDescriptor {
    ActionDescriptor::new(
        ActionName::new("http.request").expect("a valid Action name"),
        "Sends one HTTP request and returns the response's status, headers and body (parsed \
         when it is JSON). 2xx statuses and any listed in `success_statuses` succeed; 5xx, \
         408 and 429 statuses, network errors and timeouts fail as retryable; other statuses \
         fail as fatal. The Step's Idempotency Key is sent as the `Idempotency-Key` header.",
        Effect::SideEffecting,
    )
    .param_schema(schema_for::<Params>())
    .output_schema(schema_for::<Response>())
    .example(ActionExample::new(
        "Check that a site is up",
        object(json!({
            "url": "https://example.com/health",
            "timeout": "10s"
        })),
        json!({
            "status": 200,
            "headers": { "content-type": "application/json" },
            "body": { "status": "ok" },
            "body_encoding": "json"
        }),
    ))
    .example(ActionExample::new(
        "Create a resource with a bearer token",
        object(json!({
            "method": "POST",
            "url": "https://api.example.com/v1/orders",
            "auth": { "type": "bearer", "token": "{{ secrets.API_TOKEN }}" },
            "json": { "sku": "tea", "quantity": 2 }
        })),
        json!({
            "status": 201,
            "headers": {
                "content-type": "application/json",
                "location": "/v1/orders/ord_1"
            },
            "body": { "id": "ord_1", "sku": "tea", "quantity": 2 },
            "body_encoding": "json"
        }),
    ))
    .example(ActionExample::new(
        "Look up a record that may not exist",
        object(json!({
            "url": "https://api.example.com/v1/users/42",
            "query": { "fields": "name,email" },
            "headers": { "accept": "text/plain" },
            "success_statuses": [404]
        })),
        json!({
            "status": 404,
            "headers": { "content-type": "text/plain; charset=utf-8" },
            "body": "not found",
            "body_encoding": "text"
        }),
    ))
}
