//! HTTP API, OpenAPI, Scopes, webhooks and the MCP server.
//!
//! Route naming, versioning and the error format are ADR-0042: resources live under `/v1`,
//! probes and `/openapi.json` at the root, and every failure is an [`ErrorBody`]. The
//! OpenAPI document is generated from the handlers and committed as `openapi.json` next to
//! this crate's manifest; a test fails when the two drift (`just openapi` rewrites it).
//!
//! Every route but the probes and bootstrapping needs a Token, whose Scope the handler checks
//! through the extractors in [`auth`] (ADR-0043).

use std::fmt;
use std::sync::Arc;

use axum::Router;
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::get;
use bia_core::action::{ActionRegistry, CancellationToken};
use bia_core::clock::Clock;
use bia_store::secret::MasterKey;
use sqlx::PgPool;
use tokio::net::TcpListener;
use utoipa::openapi::security::{Http, HttpAuthScheme, SecurityScheme};
use utoipa::{Modify, OpenApi};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub mod auth;
mod cors;
mod error;
mod extract;
mod mcp;
mod resources;
mod routes;

pub use auth::BootstrapKey;
pub use cors::{CorsOrigins, InvalidOrigin};
pub use error::{ApiError, ErrorBody, ErrorCode};
pub use extract::{Json, Path, Query};
pub use resources::{QueueDirectory, QueueNames};

/// What every handler shares.
#[derive(Clone)]
pub struct ApiState {
    pool: PgPool,
    actions: Arc<dyn ActionRegistry>,
    clock: Arc<dyn Clock>,
    queues: Arc<dyn QueueDirectory>,
    config: Arc<ApiConfig>,
}

#[derive(Debug, Clone, Default)]
pub struct ApiConfig {
    pub cors_origins: CorsOrigins,
    /// Turns on `POST /v1/tokens/bootstrap`, which mints the first admin Token.
    pub bootstrap_key: Option<BootstrapKey>,
    /// Encrypts Secrets. Without it, the Secret routes that write answer `master_key_missing`.
    pub master_key: Option<MasterKey>,
}

impl ApiState {
    pub fn new(pool: PgPool, actions: Arc<dyn ActionRegistry>, clock: Arc<dyn Clock>) -> Self {
        Self {
            pool,
            actions,
            clock,
            queues: Arc::new(QueueNames::default()),
            config: Arc::default(),
        }
    }

    /// Where validation looks up the Queues a Definition names.
    pub fn with_queues(mut self, queues: Arc<dyn QueueDirectory>) -> Self {
        self.queues = queues;
        self
    }

    pub fn with_config(mut self, config: ApiConfig) -> Self {
        self.config = Arc::new(config);
        self
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
    components(schemas(ErrorBody, routes::workflow::TargetName)),
    modifiers(&SecuritySchemes)
)]
struct ApiDoc;

struct SecuritySchemes;

impl Modify for SecuritySchemes {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let bearer = |description: &str| {
            let mut http = Http::new(HttpAuthScheme::Bearer);
            http.description = Some(description.to_owned());
            SecurityScheme::Http(http)
        };
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "token",
            bearer(
                "A Token, `bia_` and 64 hex digits. Its Scope decides which routes it may \
                 call: `admin` everything, `agent` all but publishing, activation and \
                 administration, `webhook` only its own webhook Trigger.",
            ),
        );
        components.add_security_scheme(
            "bootstrap_key",
            bearer("The key the server was started with in `BIA_BOOTSTRAP_KEY`."),
        );
    }
}

fn documented() -> OpenApiRouter<ApiState> {
    OpenApiRouter::with_openapi(ApiDoc::openapi())
        .routes(routes!(routes::probe::health))
        .routes(routes!(routes::probe::ready))
        .routes(routes!(routes::schema::get_schema))
        .routes(routes!(
            routes::token::create_token,
            routes::token::list_tokens
        ))
        .routes(routes!(routes::token::revoke_token))
        .routes(routes!(routes::token::bootstrap_token))
        .routes(routes!(routes::secret::list_secrets))
        .routes(routes!(
            routes::secret::put_secret,
            routes::secret::delete_secret
        ))
        .routes(routes!(routes::workflow::list_workflows))
        .routes(routes!(routes::workflow::get_workflow))
        .routes(routes!(
            routes::workflow::list_versions,
            routes::workflow::create_draft
        ))
        .routes(routes!(
            routes::workflow::get_version,
            routes::workflow::replace_draft
        ))
        .routes(routes!(routes::workflow::validate_version))
        .routes(routes!(routes::workflow::publish_version))
        .routes(routes!(routes::workflow::activate_version))
        .routes(routes!(routes::workflow::diff_versions))
        .routes(routes!(routes::workflow::validate_definition))
        .routes(routes!(routes::dry_run::dry_run_version))
        .routes(routes!(routes::dry_run::dry_run_definition))
        .routes(routes!(routes::run::trigger_run))
        .routes(routes!(routes::run::list_runs))
        .routes(routes!(routes::run::list_failures))
        .routes(routes!(routes::run::get_run))
        .routes(routes!(routes::run::cancel_run))
        .routes(routes!(routes::run::resume_run))
        .routes(routes!(routes::run::retry_run))
}

/// The OpenAPI document of every route [`router`] serves.
pub fn openapi() -> utoipa::openapi::OpenApi {
    documented().split_for_parts().1
}

/// The whole API, and the MCP server at `/mcp` whose tools call it. Unknown routes and methods
/// answer with the uniform error body too. Browsers may call it from the configured CORS
/// origins only.
pub fn router(state: ApiState) -> Router {
    let cors = state.config.cors_origins.layer();
    let api = api(state.clone());
    api.clone()
        .merge(mcp::router(&state, api))
        // Outermost, so a preflight is answered before routing or authentication.
        .layer(cors)
}

fn api(state: ApiState) -> Router {
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
