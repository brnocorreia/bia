//! `http.request` through the `Action` trait, against `wiremock`.

mod common;

use std::time::Duration;

use bia_core::action::{CancellationToken, Effect, ErrorClass};
use common::{IDEMPOTENCY_KEY, action, call, execute};
use serde_json::json;
use tokio::net::TcpListener;
use wiremock::matchers::{body_json, body_string, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn returns_status_headers_and_parsed_json_body() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/health"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-request-id", "abc")
                .set_body_json(json!({ "status": "ok", "checks": [1, 2] })),
        )
        .mount(&server)
        .await;

    let output = execute(
        "http.request",
        json!({ "url": format!("{}/health", server.uri()) }),
    )
    .await
    .unwrap();

    assert_eq!(output["status"], 200);
    assert_eq!(output["headers"]["x-request-id"], "abc");
    assert_eq!(output["headers"]["content-type"], "application/json");
    assert_eq!(output["body"], json!({ "status": "ok", "checks": [1, 2] }));
    assert_eq!(output["body_encoding"], "json");
}

#[tokio::test]
async fn non_json_bodies_are_text_or_base64() {
    let server = MockServer::start().await;
    Mock::given(path("/text"))
        .respond_with(ResponseTemplate::new(200).set_body_string("hello"))
        .mount(&server)
        .await;
    Mock::given(path("/binary"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0xff, 0x00, 0xfe]))
        .mount(&server)
        .await;
    Mock::given(path("/empty"))
        .respond_with(ResponseTemplate::new(204).insert_header("content-type", "application/json"))
        .mount(&server)
        .await;

    let text = execute(
        "http.request",
        json!({ "url": format!("{}/text", server.uri()) }),
    )
    .await
    .unwrap();
    assert_eq!(text["body"], "hello");
    assert_eq!(text["body_encoding"], "text");

    let binary = execute(
        "http.request",
        json!({ "url": format!("{}/binary", server.uri()) }),
    )
    .await
    .unwrap();
    assert_eq!(binary["body"], "/wD+");
    assert_eq!(binary["body_encoding"], "base64");

    let empty = execute(
        "http.request",
        json!({ "url": format!("{}/empty", server.uri()) }),
    )
    .await
    .unwrap();
    assert_eq!(empty["status"], 204);
    assert_eq!(empty["body"], "");
    assert_eq!(empty["body_encoding"], "text");
}

#[tokio::test]
async fn forwards_the_idempotency_key() {
    let server = MockServer::start().await;
    Mock::given(header("idempotency-key", IDEMPOTENCY_KEY))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;

    execute("http.request", json!({ "url": server.uri() }))
        .await
        .unwrap();
}

#[tokio::test]
async fn sends_method_headers_query_json_body_and_bearer_auth() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/orders"))
        .and(query_param("existing", "1"))
        .and(query_param("dry_run", "true"))
        .and(query_param("limit", "5"))
        .and(header("x-tenant", "acme"))
        .and(header("authorization", "Bearer t0ken"))
        .and(header("content-type", "application/json"))
        .and(body_json(json!({ "sku": "tea", "quantity": 2 })))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({ "id": "ord_1" })))
        .expect(1)
        .mount(&server)
        .await;

    let output = execute(
        "http.request",
        json!({
            "method": "POST",
            "url": format!("{}/v1/orders?existing=1", server.uri()),
            "headers": { "X-Tenant": "acme" },
            "query": { "dry_run": true, "limit": 5 },
            "auth": { "type": "bearer", "token": "t0ken" },
            "json": { "sku": "tea", "quantity": 2 }
        }),
    )
    .await
    .unwrap();
    assert_eq!(output["status"], 201);
    assert_eq!(output["body"], json!({ "id": "ord_1" }));
}

#[tokio::test]
async fn sends_a_text_body_with_basic_auth() {
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(header("authorization", "Basic dXNlcjpwYXNz"))
        .and(header("content-type", "text/csv"))
        .and(body_string("a,b\n1,2\n"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;

    execute(
        "http.request",
        json!({
            "method": "PUT",
            "url": server.uri(),
            "headers": { "content-type": "text/csv" },
            "auth": { "type": "basic", "username": "user", "password": "pass" },
            "text": "a,b\n1,2\n"
        }),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn server_errors_408_and_429_are_retryable_and_other_client_errors_fatal() {
    let server = MockServer::start().await;
    for status in [500, 502, 503, 408, 429, 400, 401, 404, 409, 422] {
        Mock::given(path(format!("/{status}")))
            .respond_with(ResponseTemplate::new(status).set_body_json(json!({ "error": "nope" })))
            .mount(&server)
            .await;
    }

    for (status, class) in [
        (500, ErrorClass::Retryable),
        (502, ErrorClass::Retryable),
        (503, ErrorClass::Retryable),
        (408, ErrorClass::Retryable),
        (429, ErrorClass::Retryable),
        (400, ErrorClass::Fatal),
        (401, ErrorClass::Fatal),
        (404, ErrorClass::Fatal),
        (409, ErrorClass::Fatal),
        (422, ErrorClass::Fatal),
    ] {
        let error = execute(
            "http.request",
            json!({ "url": format!("{}/{status}", server.uri()) }),
        )
        .await
        .unwrap_err();
        assert_eq!(error.class(), class, "{status}");
        let details = error.details().unwrap();
        assert_eq!(details["status"], status);
        assert_eq!(details["body"], json!({ "error": "nope" }));
    }
}

#[tokio::test]
async fn success_statuses_turn_an_error_status_into_an_output() {
    let server = MockServer::start().await;
    Mock::given(path("/users/42"))
        .respond_with(ResponseTemplate::new(404).set_body_string("not found"))
        .mount(&server)
        .await;

    let output = execute(
        "http.request",
        json!({ "url": format!("{}/users/42", server.uri()), "success_statuses": [404] }),
    )
    .await
    .unwrap();
    assert_eq!(output["status"], 404);
    assert_eq!(output["body"], "not found");
}

#[tokio::test]
async fn connection_refused_is_retryable() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);

    let error = execute(
        "http.request",
        json!({ "url": format!("http://{address}/") }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.class(), ErrorClass::Retryable, "{error}");
}

#[tokio::test]
async fn a_timeout_is_retryable() {
    let server = MockServer::start().await;
    Mock::given(path("/slow"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(30)))
        .mount(&server)
        .await;

    // The timeout is enforced by the HTTP client's own timer, which the test Clock does not
    // drive, so the test waits for a short real timeout.
    let error = execute(
        "http.request",
        json!({ "url": format!("{}/slow", server.uri()), "timeout": "50ms" }),
    )
    .await
    .unwrap_err();
    assert_eq!(error.class(), ErrorClass::Retryable);
    assert!(error.message().contains("timed out"), "{error}");
}

#[tokio::test]
async fn cancellation_aborts_an_in_flight_request() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (accepted, connected) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        accepted.send(()).unwrap();
        let mut buffer = [0; 1024];
        let mut socket = socket;
        while tokio::io::AsyncReadExt::read(&mut socket, &mut buffer)
            .await
            .is_ok_and(|read| read > 0)
        {}
    });

    let cancel = CancellationToken::new();
    let request = {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            action("http.request")
                .execute(call(json!({ "url": format!("http://{address}/") })), cancel)
                .await
        })
    };
    connected.await.unwrap();
    cancel.cancel();

    let result = tokio::time::timeout(Duration::from_secs(5), request)
        .await
        .expect("the request returns promptly once cancelled")
        .unwrap();
    assert!(result.is_err());
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("the connection is closed once cancelled")
        .unwrap();
}

#[tokio::test]
async fn invalid_params_are_fatal() {
    for params in [
        json!({}),
        json!({ "url": "not a url" }),
        json!({ "url": "ftp://example.com/file" }),
        json!({ "url": "https://example.com", "method": "FETCH" }),
        json!({ "url": "https://example.com", "timeout": "soon" }),
        json!({ "url": "https://example.com", "json": {}, "text": "both" }),
        json!({ "url": "https://example.com", "auth": { "type": "digest" } }),
        json!({ "url": "https://example.com", "headers": { "bad header": "x" } }),
        json!({ "url": "https://example.com", "success_statuses": [42] }),
        json!({ "url": "https://example.com", "unknown": true }),
    ] {
        let error = execute("http.request", params.clone()).await.unwrap_err();
        assert_eq!(error.class(), ErrorClass::Fatal, "{params}: {error}");
    }
}

#[tokio::test]
async fn errors_never_echo_param_values() {
    let error = execute(
        "http.request",
        json!({ "url": "https://example.com", "headers": { "authorization": "s3cr3t\n" } }),
    )
    .await
    .unwrap_err();
    assert!(!error.to_string().contains("s3cr3t"), "{error}");

    let error = execute("http.request", json!({ "url": "s3cr3t" }))
        .await
        .unwrap_err();
    assert!(!error.to_string().contains("s3cr3t"), "{error}");

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let error = execute(
        "http.request",
        json!({ "url": format!("http://{address}/?token=s3cr3t") }),
    )
    .await
    .unwrap_err();
    assert!(!error.to_string().contains("s3cr3t"), "{error}");
}

#[test]
fn is_side_effecting() {
    assert_eq!(
        action("http.request").descriptor().effect,
        Effect::SideEffecting
    );
}
