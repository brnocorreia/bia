//! The loopback redirect of the authorization code flow (RFC 8252 section 7.3): a one-shot
//! HTTP listener on `127.0.0.1` that the browser is sent back to with the code.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{anyhow, bail};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const MAX_REQUEST: usize = 16 * 1024;
const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// Listens on a free loopback port. The redirect URI is that port's `/callback`, which the
/// built-in client's registered redirect matches whatever the port.
pub(crate) async fn bind() -> anyhow::Result<(TcpListener, String)> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let redirect_uri = format!(
        "http://127.0.0.1:{}/callback",
        listener.local_addr()?.port()
    );
    Ok((listener, redirect_uri))
}

/// Waits for the browser to arrive at `/callback` with the authorization response, and
/// returns its `code`. A request that is not that response, or whose `state` is not ours, is
/// answered and ignored, so a stray page cannot end the login; a response that names another
/// `iss` than `issuer` is refused (RFC 9207), and an OAuth `error` fails the login.
pub(crate) async fn wait_for_code(
    listener: &TcpListener,
    state: &str,
    issuer: &str,
) -> anyhow::Result<String> {
    loop {
        let (mut stream, _) = listener.accept().await?;
        let Some(target) = read_target(&mut stream).await else {
            continue;
        };
        let Some(query) = target.strip_prefix("/callback") else {
            respond(&mut stream, "404 Not Found", NOT_FOUND).await;
            continue;
        };
        let query = query.strip_prefix('?').unwrap_or_default();
        let params: HashMap<String, String> = url::form_urlencoded::parse(query.as_bytes())
            .into_owned()
            .collect();
        if params.get("state").map(String::as_str) != Some(state) {
            respond(&mut stream, "400 Bad Request", NOT_FOR_US).await;
            continue;
        }
        if params.get("iss").map(String::as_str) != Some(issuer) {
            respond(&mut stream, "400 Bad Request", FAILED).await;
            bail!("the response came from a different server than {issuer}; nothing was signed in");
        }
        if let Some(error) = params.get("error") {
            respond(&mut stream, "200 OK", DENIED).await;
            let description = params
                .get("error_description")
                .map_or(String::new(), |d| format!(": {d}"));
            bail!("sign-in was not approved ({error}{description})");
        }
        let Some(code) = params.get("code").filter(|code| !code.is_empty()) else {
            respond(&mut stream, "400 Bad Request", FAILED).await;
            return Err(anyhow!("the server's response carried no code"));
        };
        respond(&mut stream, "200 OK", DONE).await;
        return Ok(code.clone());
    }
}

/// The request target of a `GET`, or `None` for anything else or a client that stalls.
async fn read_target(stream: &mut TcpStream) -> Option<String> {
    let mut buffer = Vec::new();
    let read = async {
        let mut chunk = [0u8; 1024];
        while !buffer.windows(4).any(|w| w == b"\r\n\r\n") && buffer.len() < MAX_REQUEST {
            let n = stream.read(&mut chunk).await.ok()?;
            if n == 0 {
                return None;
            }
            buffer.extend_from_slice(&chunk[..n]);
        }
        Some(())
    };
    tokio::time::timeout(READ_TIMEOUT, read).await.ok()??;
    let text = String::from_utf8_lossy(&buffer);
    let mut parts = text.lines().next()?.split_whitespace();
    (parts.next()? == "GET").then(|| parts.next().map(str::to_owned))?
}

async fn respond(stream: &mut TcpStream, status: &str, body: &str) {
    // Fixed pages only: nothing from the request is echoed back.
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

macro_rules! page {
    ($message:literal) => {
        concat!(
            "<!doctype html><meta charset=utf-8><title>bia</title>",
            "<body style=\"font-family:system-ui;margin:3rem\"><h1>bia</h1><p>",
            $message,
            "</p>"
        )
    };
}

const DONE: &str = page!("Signed in. You can close this tab and return to the terminal.");
const DENIED: &str = page!("Sign-in was not approved. Return to the terminal.");
const FAILED: &str = page!("Sign-in failed. Return to the terminal.");
const NOT_FOUND: &str = page!("Nothing here.");
const NOT_FOR_US: &str = page!("This is not the response bia is waiting for.");
