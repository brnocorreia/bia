//! HTTP API, OpenAPI, Scopes, webhooks and the MCP server.
//!
//! Route naming, versioning and the error format are ADR-0042: resources live under `/v1`,
//! probes and `/openapi.json` at the root, and every failure is an [`ErrorBody`]. The
//! OpenAPI document is generated from the handlers and committed as `openapi.json` next to
//! this crate's manifest; a test fails when the two drift (`just openapi` rewrites it).
//!
//! Every route but the probes, first-run setup and sign-in needs a Token, whose Scope the
//! handler checks through the extractors in [`auth`] (ADR-0043). A browser holds its Token in
//! the Session cookie (ADR-0064). The one exception is a webhook Trigger with
//! `hmac`, whose callers sign the body instead (ADR-0058).

use std::fmt;
use std::net::SocketAddr;
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
use utoipa::openapi::security::{ApiKey, ApiKeyValue, Http, HttpAuthScheme, SecurityScheme};
use utoipa::{Modify, OpenApi};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub mod auth;
mod cors;
mod error;
mod extract;
mod mcp;
mod oauth;
mod routes;
#[cfg(feature = "ui")]
mod ui;

pub use auth::{SetupCode, Wait};
pub use cors::{CorsOrigins, InvalidOrigin};
pub use error::{ApiError, ErrorBody, ErrorCode};
pub use extract::{Json, Path, Query};
pub use oauth::{ClientMetadataFetcher, Resolve};

/// What every handler shares.
#[derive(Clone)]
pub struct ApiState {
    pool: PgPool,
    actions: Arc<dyn ActionRegistry>,
    clock: Arc<dyn Clock>,
    config: Arc<ApiConfig>,
    sign_ins: Arc<auth::SignInThrottle>,
    registrations: Arc<oauth::register::Limiter>,
    device_requests: Arc<oauth::register::Limiter>,
    metadata: Arc<oauth::MetadataCache>,
}

#[derive(Debug, Clone, Default)]
pub struct ApiConfig {
    /// Origins other than `ui_url`'s that may call the API from a browser, without credentials.
    pub cors_origins: CorsOrigins,
    /// Where people reach the API. Its origin is accepted on requests made with the Session
    /// cookie, and an `https` URL makes the cookie `Secure`.
    pub public_url: Option<url::Url>,
    /// Where the UI is. Its origin is accepted like `public_url`'s, and is the only one allowed
    /// to send the Session cookie from another site.
    pub ui_url: Option<url::Url>,
    /// How sign-in waits out its throttle. Sleeps unless a test says otherwise.
    pub sign_in_wait: Option<Arc<dyn Wait>>,
    /// The code `POST /v1/setup` takes, from the Setup Link. Without one, setup refuses every
    /// code.
    pub setup_code: Option<SetupCode>,
    /// How a client's metadata document is fetched when its `client_id` is a URL. Strict by
    /// default: HTTPS to public addresses only. Nothing configures it; only a test built with
    /// the `testing` feature can swap in a relaxed one (ADR-0067).
    pub client_metadata_fetcher: ClientMetadataFetcher,
    /// How long a device must leave between polls of the token endpoint. Five seconds unless a
    /// test says otherwise; nothing configures it.
    pub device_poll_interval: Option<std::time::Duration>,
    /// Encrypts Secrets. Without it, the Secret routes that write answer `master_key_missing`.
    pub master_key: Option<MasterKey>,
}

impl ApiState {
    pub fn new(pool: PgPool, actions: Arc<dyn ActionRegistry>, clock: Arc<dyn Clock>) -> Self {
        Self {
            pool,
            actions,
            clock,
            config: Arc::default(),
            sign_ins: Arc::default(),
            registrations: Arc::default(),
            device_requests: Arc::default(),
            metadata: Arc::default(),
        }
    }

    pub fn with_config(mut self, config: ApiConfig) -> Self {
        self.config = Arc::new(config);
        self
    }
}

impl ApiConfig {
    /// Whether `origin`, as a browser's `Origin` header writes it, is `public_url`'s or
    /// `ui_url`'s.
    pub(crate) fn is_own_origin(&self, origin: &str) -> bool {
        [&self.public_url, &self.ui_url]
            .into_iter()
            .flatten()
            .any(|url| url.origin().ascii_serialization() == origin)
    }

    /// Whether the Session cookie is `Secure`: `public_url` is HTTPS.
    pub(crate) fn secure_cookies(&self) -> bool {
        self.public_url
            .as_ref()
            .is_some_and(|url| url.scheme() == "https")
    }

    fn ui_origin(&self) -> Option<axum::http::HeaderValue> {
        self.ui_url
            .as_ref()
            .and_then(|url| url.origin().ascii_serialization().parse().ok())
    }

    pub(crate) fn wait(&self) -> &dyn Wait {
        self.sign_in_wait.as_deref().unwrap_or(&auth::TokioWait)
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
            "session",
            SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::with_description(
                "bia_session",
                "The Session cookie `POST /v1/session` sets: an `admin` Token for browsers. \
                 Wherever `token` is accepted, this is too, except on `/mcp`. A request with an \
                 unsafe method must carry an `Origin` equal to the origin of `public_url` or \
                 `ui_url`, or it fails with `origin_mismatch`.",
            ))),
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
        .routes(routes!(routes::token::current_token))
        .routes(routes!(routes::token::revoke_token))
        .routes(routes!(routes::setup::set_up))
        .routes(routes!(routes::session::sign_in, routes::session::sign_out))
        .routes(routes!(routes::session::sign_out_others))
        .routes(routes!(routes::user::change_password))
        .routes(routes!(routes::grant::list_grants))
        .routes(routes!(routes::grant::revoke_grant))
        .routes(routes!(oauth::discovery::api_resource_metadata))
        .routes(routes!(oauth::discovery::mcp_resource_metadata))
        .routes(routes!(oauth::discovery::authorization_server_metadata))
        .routes(routes!(oauth::authorize::authorize))
        .routes(routes!(oauth::token::token))
        .routes(routes!(oauth::revoke::revoke))
        .routes(routes!(oauth::register::register))
        .routes(routes!(oauth::consent::get_consent))
        .routes(routes!(oauth::consent::approve))
        .routes(routes!(oauth::consent::deny))
        .routes(routes!(oauth::device::device_authorization))
        .routes(routes!(oauth::device::lookup))
        .routes(routes!(oauth::device::get_device_consent))
        .routes(routes!(oauth::device::approve_device))
        .routes(routes!(oauth::device::deny_device))
        .routes(routes!(routes::secret::list_secrets))
        .routes(routes!(
            routes::secret::put_secret,
            routes::secret::delete_secret
        ))
        .routes(routes!(routes::queue::list_queues))
        .routes(routes!(
            routes::queue::get_queue,
            routes::queue::put_queue,
            routes::queue::delete_queue
        ))
        .routes(routes!(routes::queue::list_messages))
        .routes(routes!(routes::queue::replay_dead_letters))
        .routes(routes!(routes::queue::purge_dead_letters))
        .routes(routes!(
            routes::approval::create_approval_request,
            routes::approval::list_approval_requests
        ))
        .routes(routes!(routes::approval::get_approval_request))
        .routes(routes!(routes::approval::withdraw_approval_request))
        .routes(routes!(routes::approval::approve_approval_request))
        .routes(routes!(routes::approval::reject_approval_request))
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
        .routes(routes!(routes::webhook::trigger_webhook))
}

/// The OpenAPI document of every route [`router`] serves.
pub fn openapi() -> utoipa::openapi::OpenApi {
    documented().split_for_parts().1
}

/// The whole API, and the MCP server at `/mcp` whose tools call it. Unknown routes and methods
/// answer with the uniform error body too. Browsers may call it from the configured CORS
/// origins and the UI's only; the UI's may send the Session cookie.
pub fn router(state: ApiState) -> Router {
    let cors = state.config.cors_origins.layer(state.config.ui_origin());
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
        .fallback(not_found)
        .method_not_allowed_fallback(|| async { ApiError::method_not_allowed().into_response() })
        .with_state(state)
}

/// Unmatched routes answer with the uniform error body, except that the embedded UI (`ui`
/// feature) takes the screens it owns.
async fn not_found(
    method: axum::http::Method,
    uri: axum::http::Uri,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    #[cfg(feature = "ui")]
    if let Some(page) = ui::fallback(&method, &uri, &headers) {
        return page;
    }
    let _ = (&method, &uri, &headers);
    ApiError::route_not_found().into_response()
}

/// Serves `router` on `listener` until `shutdown` fires, then lets in-flight requests finish.
pub async fn serve(
    listener: TcpListener,
    router: Router,
    shutdown: CancellationToken,
) -> std::io::Result<()> {
    // The peer address is what sign-in throttling counts attempts against.
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown.cancelled_owned())
    .await
}
