//! HTTP API, OpenAPI, Scopes, webhooks and the MCP server.
//!
//! Route naming, versioning and the error format are ADR-0042: resources live under `/v1`,
//! probes and `/openapi.json` at the root, and every failure is an [`ErrorBody`]. The
//! OpenAPI document is generated from the handlers and committed as `openapi.json` next to
//! this crate's manifest; a test fails when the two drift (`just openapi` rewrites it).

use std::fmt;
use std::sync::Arc;

use axum::Router;
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::get;
use bia_core::action::{ActionRegistry, CancellationToken};
use sqlx::PgPool;
use tokio::net::TcpListener;
use utoipa::OpenApi;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

mod error;
mod extract;
mod routes;

pub use error::{ApiError, ErrorBody, ErrorCode};
pub use extract::Json;

/// What every handler shares.
#[derive(Clone)]
pub struct ApiState {
    pool: PgPool,
    actions: Arc<dyn ActionRegistry>,
}

impl ApiState {
    pub fn new(pool: PgPool, actions: Arc<dyn ActionRegistry>) -> Self {
        Self { pool, actions }
    }
}

impl fmt::Debug for ApiState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ApiState").finish_non_exhaustive()
    }
}

#[derive(OpenApi)]
#[openapi(
    info(
        title = "bia",
        description = "Workflows as data, run from Postgres. Every endpoint fails with the same \
                       error body: a stable `code`, a `message`, and `diagnostics` when a \
                       Definition was at fault."
    ),
    components(schemas(ErrorBody))
)]
struct ApiDoc;

fn documented() -> OpenApiRouter<ApiState> {
    OpenApiRouter::with_openapi(ApiDoc::openapi())
        .routes(routes!(routes::probe::health))
        .routes(routes!(routes::probe::ready))
        .routes(routes!(routes::schema::get_schema))
}

/// The OpenAPI document of every route [`router`] serves.
pub fn openapi() -> utoipa::openapi::OpenApi {
    documented().split_for_parts().1
}

/// The whole API. Unknown routes and methods answer with the uniform error body too.
pub fn router(state: ApiState) -> Router {
    let (router, openapi) = documented().split_for_parts();
    let document = openapi
        .to_pretty_json()
        .unwrap_or_else(|_| unreachable!("the OpenAPI document serialises"));
    router
        .route(
            "/openapi.json",
            get(move || async move { ([(header::CONTENT_TYPE, "application/json")], document) }),
        )
        .fallback(|| async { ApiError::route_not_found().into_response() })
        .method_not_allowed_fallback(|| async { ApiError::method_not_allowed().into_response() })
        .with_state(state)
}

/// Serves `router` on `listener` until `shutdown` fires, then lets in-flight requests finish.
pub async fn serve(
    listener: TcpListener,
    router: Router,
    shutdown: CancellationToken,
) -> std::io::Result<()> {
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown.cancelled_owned())
        .await
}
