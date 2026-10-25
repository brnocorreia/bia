//! The UI bundle embedded in the binary (ADR-0013, ADR-0052).
//!
//! It is served from the site root as the router's fallback, so it only ever answers a request
//! no API route matched; an API route can never be shadowed.

use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "$OUT_DIR/ui"]
struct Bundle;

/// The page's own origin is the API: the bundle's `config.json` is not embedded, it is this.
const CONFIG: &str = r#"{"apiUrl":""}"#;

/// Scripts and styles only from the bundle (ADR-0050). `blob:` workers are for the editor.
const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self'; \
    style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self' data:; \
    worker-src 'self' blob:; connect-src 'self'; object-src 'none'; base-uri 'self'; \
    frame-ancestors 'none'";

/// What the router answers when no API route matched `uri`, or `None` to answer with the API's
/// own not-found error.
pub(crate) fn fallback(method: &Method, uri: &Uri, request: &HeaderMap) -> Option<Response> {
    if method != Method::GET && method != Method::HEAD {
        return None;
    }
    let path = uri.path();
    if path == "/v1" || path.starts_with("/v1/") || path == "/mcp" || path.starts_with("/mcp/") {
        return None;
    }
    if path == "/config.json" {
        return Some(respond(
            method,
            "application/json",
            "no-cache",
            None,
            CONFIG.as_bytes().to_vec(),
        ));
    }
    let name = path.trim_start_matches('/');
    if !name.is_empty()
        && let Some(file) = Bundle::get(name)
    {
        return Some(file_response(method, request, name, file));
    }
    // A missing file (`/assets/old.js`) is a 404; only an extension-less path is a screen.
    if name
        .rsplit('/')
        .next()
        .is_some_and(|last| last.contains('.'))
    {
        return None;
    }
    let index = Bundle::get("index.html")?;
    Some(file_response(method, request, "index.html", index))
}

fn file_response(
    method: &Method,
    request: &HeaderMap,
    name: &str,
    file: rust_embed::EmbeddedFile,
) -> Response {
    let etag = format!("\"{}\"", hex::encode(file.metadata.sha256_hash()));
    // Vite names everything under `assets/` after its content, so it never changes.
    let cache = if name.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    if request
        .get(header::IF_NONE_MATCH)
        .is_some_and(|sent| sent.as_bytes() == etag.as_bytes())
    {
        let mut response = StatusCode::NOT_MODIFIED.into_response();
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
        return response;
    }
    respond(
        method,
        file.metadata.mimetype(),
        cache,
        Some(&etag),
        file.data.into_owned(),
    )
}

fn respond(
    method: &Method,
    content_type: &str,
    cache: &'static str,
    etag: Option<&str>,
    body: Vec<u8>,
) -> Response {
    let mut response = Response::new(if method == Method::HEAD {
        Body::empty()
    } else {
        Body::from(body)
    });
    let headers = response.headers_mut();
    let mut set = |name, value: &str| {
        if let Ok(value) = HeaderValue::from_str(value) {
            headers.insert(name, value);
        }
    };
    set(header::CONTENT_TYPE, content_type);
    set(header::CACHE_CONTROL, cache);
    set(header::CONTENT_SECURITY_POLICY, CONTENT_SECURITY_POLICY);
    set(header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    if let Some(etag) = etag {
        set(header::ETAG, etag);
    }
    response
}
