//! MCP over streamable HTTP at `/mcp` (ADR-0048).
//!
//! The MCP server holds no logic of its own: each tool is built into an API request, which
//! the API router answers as it would any client's, with the caller's own `Authorization`
//! header. Authentication, Scopes and validation are therefore the API's, and a tool can do
//! nothing its endpoint cannot.

use std::borrow::Cow;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::{FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderValue, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use rmcp::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ErrorData, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool, ToolAnnotations,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use serde_json::{Map, Value, json};
use tower::ServiceExt;

use crate::ApiState;
use crate::auth::Agent;

mod tools;

use tools::{Call, ToolDef};

const INSTRUCTIONS: &str = "bia runs Workflows defined as data. To author one: read \
`get_schema`, write a Definition, check it with `validate_workflow` (fix every Diagnostic: \
each has a path, a code and a suggestion), save it with `create_draft`, then try it with \
`dry_run` and Mocks. Publishing and activating a Version need an admin and are not available \
here: ask a human. A failed tool call carries the API's error body: a stable `code`, a \
`message` and, for a bad Definition, `diagnostics`.";

/// The `/mcp` route, whose tools send their requests to `api`.
///
/// Every request carries its own Token and is checked before MCP sees it, and again by `api`
/// for each tool call, so a session never outlives a revoked Token.
pub(crate) fn router(state: &ApiState, api: Router) -> Router {
    let config = StreamableHttpServerConfig::default()
        // The Token, not the network location, authenticates a caller, and bia is served
        // under whatever host name an operator picks.
        .disable_allowed_hosts()
        // Requests are independent, so any API replica can serve any of them.
        .with_legacy_session_mode(false)
        .with_json_response(true);
    let service = StreamableHttpService::new(
        move || Ok(Mcp { api: api.clone() }),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    Router::new()
        .nest_service("/mcp", service)
        .layer(middleware::from_fn_with_state(state.clone(), authenticate))
}

/// Refuses a request that carries no Token an agent may use, before MCP sees it.
async fn authenticate(State(state): State<ApiState>, request: Request, next: Next) -> Response {
    let (mut parts, body) = request.into_parts();
    match Agent::from_request_parts(&mut parts, &state).await {
        Ok(_) => next.run(Request::from_parts(parts, body)).await,
        Err(error) => error.into_response(),
    }
}

#[derive(Clone)]
struct Mcp {
    api: Router,
}

impl Mcp {
    async fn dispatch(&self, call: Call, authorization: HeaderValue) -> CallToolResult {
        let mut request = Request::builder()
            .method(call.method)
            .uri(&call.uri)
            .header(header::AUTHORIZATION, authorization);
        let body = match &call.body {
            Some(body) => {
                request = request.header(header::CONTENT_TYPE, "application/json");
                Body::from(body.to_string())
            }
            None => Body::empty(),
        };
        let Ok(request) = request.body(body) else {
            return failure(
                "malformed_request",
                "the arguments do not form a valid request",
            );
        };
        let response = match self.api.clone().oneshot(request).await {
            Ok(response) => response,
            Err(never) => match never {},
        };
        let status = response.status();
        let Ok(bytes) = axum::body::to_bytes(response.into_body(), usize::MAX).await else {
            return failure("internal", "the API response could not be read");
        };
        let body = if bytes.is_empty() {
            json!({})
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(
                |_| json!({ "code": "internal", "message": String::from_utf8_lossy(&bytes) }),
            )
        };
        if !status.is_success() {
            return CallToolResult::structured_error(body);
        }
        match call.only {
            Some(key) => CallToolResult::structured(json!({ key: body[key] })),
            None => CallToolResult::structured(body),
        }
    }
}

fn failure(code: &str, message: &str) -> CallToolResult {
    CallToolResult::structured_error(json!({ "code": code, "message": message }))
}

fn definition(def: &ToolDef) -> Tool {
    let Value::Object(schema) = (def.schema)() else {
        unreachable!("tool input schemas are objects")
    };
    let annotations = ToolAnnotations::new()
        .read_only(def.read_only)
        .destructive(false)
        .open_world(false);
    Tool::new(def.name, Cow::Borrowed(def.description), schema).annotate(annotations)
}

impl ServerHandler for Mcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(INSTRUCTIONS)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(
            tools::TOOLS.iter().map(definition).collect(),
        ))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let Some(def) = tools::TOOLS.iter().find(|def| def.name == request.name) else {
            return Err(ErrorData::invalid_params(
                format!("there is no tool `{}`", request.name),
                None,
            ));
        };
        let authorization = context
            .extensions
            .get::<Parts>()
            .and_then(|parts| parts.headers.get(header::AUTHORIZATION))
            .cloned()
            .ok_or_else(|| {
                ErrorData::invalid_request("the request carries no `Authorization` header", None)
            })?;
        let arguments: Map<String, Value> = request.arguments.unwrap_or_default();
        let result = match (def.build)(&arguments) {
            Ok(call) => self.dispatch(call, authorization).await,
            Err(message) => failure("malformed_request", &message),
        };
        Ok(result.into())
    }
}
