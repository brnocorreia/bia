//! `bia mcp`: a stdio proxy to the server's `/mcp` endpoint, for MCP clients that spawn a
//! local process (ADR-0048). It adds nothing: each JSON-RPC message read from stdin is POSTed
//! with the configured Token, and what comes back is written to stdout, one message per line.

use std::sync::Arc;

use anyhow::Context;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use reqwest::{Client, Response};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

const SESSION_ID: &str = "mcp-session-id";
const PROTOCOL_VERSION: &str = "mcp-protocol-version";

/// Proxies stdin to `{url}/mcp` until stdin closes.
pub(crate) async fn proxy(url: &str, token: Option<&str>) -> anyhow::Result<()> {
    let endpoint = format!("{}/mcp", url.trim_end_matches('/'));
    reqwest::Url::parse(&endpoint).with_context(|| format!("invalid server URL `{url}`"))?;
    run(
        tokio::io::stdin(),
        tokio::io::stdout(),
        endpoint,
        token.map(str::to_owned),
    )
    .await
}

async fn run(
    input: impl tokio::io::AsyncRead + Unpin,
    output: impl AsyncWrite + Unpin + Send + 'static,
    endpoint: String,
    token: Option<String>,
) -> anyhow::Result<()> {
    let proxy = Arc::new(Proxy {
        http: Client::new(),
        endpoint,
        token,
        headers: Mutex::default(),
        output: Mutex::new(output),
    });
    let mut lines = BufReader::new(input).lines();
    let mut tasks = tokio::task::JoinSet::new();
    while let Some(line) = lines.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let proxy = proxy.clone();
        tasks.spawn(async move { proxy.forward(&line).await });
    }
    while tasks.join_next().await.is_some() {}
    Ok(())
}

struct Proxy<W> {
    http: Client,
    endpoint: String,
    token: Option<String>,
    /// What the server handed out and expects back: the session and the protocol version.
    headers: Mutex<Vec<(&'static str, String)>>,
    output: Mutex<W>,
}

impl<W: AsyncWrite + Unpin> Proxy<W> {
    async fn forward(&self, message: &str) {
        let id = serde_json::from_str::<Value>(message)
            .ok()
            .and_then(|message| message.get("id").cloned());
        let replies = match self.post(message).await {
            Ok(replies) => replies,
            Err(error) => match id {
                Some(id) => vec![error_reply(id, &error)],
                None => {
                    eprintln!("bia mcp: {error}");
                    Vec::new()
                }
            },
        };
        let mut output = self.output.lock().await;
        for reply in replies {
            let line = format!("{reply}\n");
            if output.write_all(line.as_bytes()).await.is_err() || output.flush().await.is_err() {
                return;
            }
        }
    }

    async fn post(&self, message: &str) -> Result<Vec<Value>, String> {
        let mut request = self
            .http
            .post(&self.endpoint)
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json, text/event-stream")
            .body(message.to_owned());
        if let Some(token) = &self.token {
            request = request.header(AUTHORIZATION, format!("Bearer {token}"));
        }
        for (name, value) in self.headers.lock().await.iter() {
            request = request.header(*name, value);
        }
        let response = request
            .send()
            .await
            .map_err(|e| format!("cannot reach {}: {e}", self.endpoint))?;
        self.remember(&response).await;
        let status = response.status();
        let is_stream = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("text/event-stream"));
        let body = response
            .text()
            .await
            .map_err(|e| format!("cannot read the response: {e}"))?;
        if !status.is_success() {
            return Err(failure(status.as_u16(), &body));
        }
        let texts: Vec<String> = if is_stream {
            data_events(&body)
        } else if body.trim().is_empty() {
            Vec::new()
        } else {
            vec![body]
        };
        let replies: Vec<Value> = texts
            .iter()
            .filter_map(|text| serde_json::from_str(text).ok())
            .collect();
        // Stdio carries no HTTP headers, so the negotiated version is learned from the
        // `initialize` result and sent on every later request.
        let negotiated = replies
            .iter()
            .find_map(|reply| reply.pointer("/result/protocolVersion")?.as_str());
        if let Some(version) = negotiated {
            self.set(PROTOCOL_VERSION, version.to_owned()).await;
        }
        Ok(replies)
    }

    async fn set(&self, name: &'static str, value: String) {
        let mut remembered = self.headers.lock().await;
        remembered.retain(|(known, _)| *known != name);
        remembered.push((name, value));
    }

    async fn remember(&self, response: &Response) {
        for name in [SESSION_ID, PROTOCOL_VERSION] {
            if let Some(value) = response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
            {
                self.set(name, value.to_owned()).await;
            }
        }
    }
}

/// The payload of each Server-Sent Event.
fn data_events(stream: &str) -> Vec<String> {
    stream
        .replace("\r\n", "\n")
        .split("\n\n")
        .filter_map(|event| {
            let data: Vec<&str> = event
                .lines()
                .filter_map(|line| line.strip_prefix("data:"))
                .map(|data| data.strip_prefix(' ').unwrap_or(data))
                .collect();
            (!data.is_empty()).then(|| data.join("\n"))
        })
        .collect()
}

/// Why the server refused, in the API's own words when it sent its error body.
fn failure(status: u16, body: &str) -> String {
    let api = serde_json::from_str::<Value>(body).ok();
    let field = |name: &str| {
        api.as_ref()
            .and_then(|body| body.get(name))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    match (field("code"), field("message")) {
        (Some(code), Some(message)) => format!("the server answered {status} `{code}`: {message}"),
        _ => format!("the server answered {status}"),
    }
}

fn error_reply(id: Value, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32000, "message": message } })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_events_joins_multiline_data_and_skips_comments() {
        let stream = ": keep-alive\n\nid: 1\ndata: {\"a\":1}\n\ndata: x\ndata: y\n\n";
        assert_eq!(data_events(stream), ["{\"a\":1}", "x\ny"]);
    }

    #[test]
    fn failure_quotes_the_api_error_body() {
        let body = r#"{"code":"invalid_token","message":"the Token is unknown or revoked"}"#;
        assert_eq!(
            failure(401, body),
            "the server answered 401 `invalid_token`: the Token is unknown or revoked"
        );
        assert_eq!(failure(502, "<html>"), "the server answered 502");
    }
}
