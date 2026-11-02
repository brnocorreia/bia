//! `bia login` and `bia logout` (ADR-0068) against an in-process server. The "browser" is the
//! test: it follows the sign-in URL the CLI prints and answers the consent as the signed-in
//! person. Every `bia` process gets its own config directory and no OS keychain.

#![cfg(unix)]
#![allow(clippy::unwrap_used)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use bia_client::Client;
use bia_core::action::CancellationToken;
use bia_server::{ApiConfig, ApiState, SetupCode};
use bia_store::SystemClock;
use serde_json::{Value, json};
use sqlx::PgPool;
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStderr, Command};

struct Harness {
    url: String,
    session: String,
    config_dir: TempDir,
    http: reqwest::Client,
    shutdown: CancellationToken,
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

async fn start(pool: PgPool) -> Harness {
    let registry = Arc::new(bia_actions::builtin_registry().unwrap());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (setup_code, code) = SetupCode::generate().unwrap();
    let config = ApiConfig {
        setup_code: Some(setup_code),
        public_url: Some(url.parse().unwrap()),
        ui_url: Some(url.parse().unwrap()),
        device_poll_interval: Some(Duration::from_secs(1)),
        ..ApiConfig::default()
    };
    let state = ApiState::new(pool, registry, Arc::new(SystemClock)).with_config(config);
    let shutdown = CancellationToken::new();
    tokio::spawn(bia_server::serve(
        listener,
        bia_server::router(state),
        shutdown.clone(),
    ));
    let phrase = ["correct", "horse", "battery", "staple"].join("-");
    let done = Client::new(&url)
        .unwrap()
        .set_up(&code, &phrase)
        .await
        .unwrap();
    Harness {
        url,
        session: done.session,
        config_dir: TempDir::new().unwrap(),
        http: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap(),
        shutdown,
    }
}

struct Ran {
    status: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Harness {
    /// A directory that does not exist yet, so the test sees how the CLI creates it.
    fn config_path(&self) -> PathBuf {
        self.config_dir.path().join("config")
    }

    fn credentials_file(&self) -> PathBuf {
        self.config_path().join("credentials.json")
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_bia"));
        command
            .args(args)
            .env_remove("BIA_CONFIG")
            .env_remove("BIA_TOKEN")
            .env("BIA_URL", &self.url)
            .env("BIA_KEYCHAIN", "off")
            .env("BIA_CONFIG_DIR", self.config_path())
            .stdin(Stdio::null())
            .kill_on_drop(true);
        command
    }

    async fn bia(&self, args: &[&str]) -> Ran {
        self.bia_with(None, args).await
    }

    async fn bia_with(&self, token: Option<&str>, args: &[&str]) -> Ran {
        let mut command = self.command(args);
        if let Some(token) = token {
            command.env("BIA_TOKEN", token);
        }
        let output = tokio::time::timeout(Duration::from_secs(60), command.output())
            .await
            .expect("bia did not exit")
            .unwrap();
        Ran {
            status: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    /// Starts `bia login` and reads its stderr until `wanted` finds what the person is told.
    async fn start_login(&self, args: &[&str], wanted: impl Fn(&str) -> Option<String>) -> Login {
        let mut child = self
            .command(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut lines = BufReader::new(child.stderr.take().unwrap()).lines();
        let mut seen = String::new();
        let found = tokio::time::timeout(Duration::from_secs(30), async {
            while let Some(line) = lines.next_line().await.unwrap() {
                seen.push_str(&line);
                seen.push('\n');
                if let Some(found) = wanted(line.trim()) {
                    return found;
                }
            }
            panic!("bia login ended before telling the person what to do:\n{seen}");
        })
        .await
        .expect("bia login told the person nothing");
        Login {
            child,
            lines,
            seen,
            found,
        }
    }

    fn as_person(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        request
            .header("cookie", format!("bia_session={}", self.session))
            .header("origin", &self.url)
    }

    /// What a person does in the browser: open the sign-in URL, approve, and let the browser
    /// follow the redirect back to the CLI.
    async fn approve_in_browser(&self, sign_in_url: &str) {
        let response = self.http.get(sign_in_url).send().await.unwrap();
        assert_eq!(response.status(), 302, "{sign_in_url}");
        let location = response.headers()["location"].to_str().unwrap().to_owned();
        let consent = url::Url::parse(&location).unwrap();
        let (_, id) = consent
            .query_pairs()
            .find(|(name, _)| name == "request")
            .unwrap();
        let approved: Value = self
            .as_person(
                self.http
                    .post(format!("{}/oauth/consent/{id}/approve", self.url))
                    .json(&json!({})),
            )
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let callback = approved["redirect_to"].as_str().unwrap();
        assert!(callback.starts_with("http://127.0.0.1:"), "{callback}");
        let page = self.http.get(callback).send().await.unwrap();
        assert_eq!(page.status(), 200);
    }

    /// What a person does on another device: type the user code and approve.
    async fn approve_device(&self, user_code: &str, decision: &str) {
        let found: Value = self
            .as_person(
                self.http
                    .post(format!("{}/oauth/device/lookup", self.url))
                    .json(&json!({ "user_code": user_code })),
            )
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let id = found["id"].as_str().unwrap();
        assert_eq!(found["client"]["name"], "bia CLI (built in)");
        let answered = self
            .as_person(
                self.http
                    .post(format!("{}/oauth/device/{id}/{decision}", self.url))
                    .json(&json!({})),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(answered.status(), 200);
    }

    /// Logs in through the browser flow.
    async fn log_in(&self) {
        let login = self
            .start_login(&["login", "--no-browser"], |line| {
                line.starts_with("http").then(|| line.to_owned())
            })
            .await;
        self.approve_in_browser(&login.found).await;
        let ran = login.finish().await;
        assert_eq!(ran.status, Some(0), "{}", ran.stderr);
    }

    fn stored(&self) -> Value {
        let text = std::fs::read_to_string(self.credentials_file()).unwrap();
        let file: Value = serde_json::from_str(&text).unwrap();
        file["logins"][&self.url].clone()
    }

    fn rewrite_stored(&self, change: impl FnOnce(&mut Value)) {
        let mut file: Value =
            serde_json::from_str(&std::fs::read_to_string(self.credentials_file()).unwrap())
                .unwrap();
        change(&mut file["logins"][&self.url]);
        std::fs::write(self.credentials_file(), file.to_string()).unwrap();
    }
}

struct Login {
    child: Child,
    lines: tokio::io::Lines<BufReader<ChildStderr>>,
    seen: String,
    /// What the `wanted` closure found in the CLI's output.
    found: String,
}

impl Login {
    async fn finish(mut self) -> Ran {
        let rest = async {
            while let Some(line) = self.lines.next_line().await.unwrap() {
                self.seen.push_str(&line);
                self.seen.push('\n');
            }
        };
        tokio::time::timeout(Duration::from_secs(60), rest)
            .await
            .expect("bia login did not finish");
        let output = self.child.wait_with_output().await.unwrap();
        Ran {
            status: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: self.seen,
        }
    }
}

fn mode(path: &Path) -> u32 {
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// Checks that none of the stored credential's values reached what the CLI printed.
fn assert_no_credentials_in(output: &str, stored: &Value) {
    for field in ["access_token", "refresh_token"] {
        let value = stored[field].as_str().unwrap();
        assert!(!output.contains(value), "{field} was printed:\n{output}");
    }
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn login_completes_through_the_browser_and_later_commands_work_without_a_token(pool: PgPool) {
    let h = start(pool).await;
    let before = h.bia(&["workflow", "list"]).await;
    assert_eq!(before.status, Some(1));
    assert!(before.stderr.contains("bia login"), "{}", before.stderr);

    let login = h
        .start_login(&["login", "--no-browser"], |line| {
            line.starts_with("http").then(|| line.to_owned())
        })
        .await;
    assert!(login.found.contains("/oauth/authorize?"), "{}", login.found);
    assert!(login.found.contains("code_challenge_method=S256"));
    assert!(login.found.contains("client_id=bia-cli"));
    assert!(login.found.contains("scope=admin"));
    h.approve_in_browser(&login.found).await;
    let ran = login.finish().await;
    assert_eq!(ran.status, Some(0), "{}", ran.stderr);
    assert!(ran.stderr.contains("Logged in to"), "{}", ran.stderr);

    // The fallback file is private, and nothing the CLI printed holds a credential.
    assert_eq!(mode(&h.credentials_file()), 0o600);
    assert_eq!(mode(&h.config_path()), 0o700);
    let stored = h.stored();
    assert_eq!(stored["scope"], "admin");
    assert_no_credentials_in(&ran.stderr, &stored);
    assert_no_credentials_in(&ran.stdout, &stored);

    let listed = h.bia(&["workflow", "list", "--json"]).await;
    assert_eq!(listed.status, Some(0), "{}", listed.stderr);
    assert_eq!(
        serde_json::from_str::<Value>(&listed.stdout).unwrap(),
        json!({ "workflows": [] })
    );
    assert_no_credentials_in(&listed.stderr, &stored);

    let tokens = h.bia(&["token", "list"]).await;
    assert_eq!(tokens.status, Some(0), "{}", tokens.stderr);
    assert!(tokens.stdout.contains("Oauth"), "{}", tokens.stdout);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_login_that_is_not_approved_stores_nothing(pool: PgPool) {
    let h = start(pool).await;
    let login = h
        .start_login(&["login", "--no-browser"], |line| {
            line.starts_with("http").then(|| line.to_owned())
        })
        .await;
    let response = h.http.get(&login.found).send().await.unwrap();
    let location = response.headers()["location"].to_str().unwrap().to_owned();
    let id = url::Url::parse(&location)
        .unwrap()
        .query_pairs()
        .find(|(name, _)| name == "request")
        .unwrap()
        .1
        .into_owned();
    let denied: Value = h
        .as_person(
            h.http
                .post(format!("{}/oauth/consent/{id}/deny", h.url))
                .json(&json!({})),
        )
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    h.http
        .get(denied["redirect_to"].as_str().unwrap())
        .send()
        .await
        .unwrap();

    let ran = login.finish().await;

    assert_eq!(ran.status, Some(1));
    assert!(ran.stderr.contains("access_denied"), "{}", ran.stderr);
    assert!(!h.credentials_file().exists());
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn the_cli_ignores_a_callback_that_is_not_its_own_and_still_finishes(pool: PgPool) {
    let h = start(pool).await;
    let login = h
        .start_login(&["login", "--no-browser"], |line| {
            line.starts_with("http").then(|| line.to_owned())
        })
        .await;
    let redirect = url::Url::parse(&login.found)
        .unwrap()
        .query_pairs()
        .find(|(name, _)| name == "redirect_uri")
        .unwrap()
        .1
        .into_owned();
    // A page that does not know the state cannot end the login or get its code.
    let stray = h
        .http
        .get(format!("{redirect}?code=forged&state=wrong&iss={}", h.url))
        .send()
        .await
        .unwrap();
    assert_eq!(stray.status(), 400);

    h.approve_in_browser(&login.found).await;
    let ran = login.finish().await;

    assert_eq!(ran.status, Some(0), "{}", ran.stderr);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn device_login_waits_for_the_person_and_then_stores_a_login(pool: PgPool) {
    let h = start(pool).await;
    let login = h
        .start_login(&["login", "--device"], |line| {
            let code = line.trim();
            let looks_like_a_code = code.len() == 9
                && code.as_bytes()[4] == b'-'
                && code.chars().all(|c| c == '-' || c.is_ascii_uppercase());
            looks_like_a_code.then(|| code.to_owned())
        })
        .await;
    assert!(login.seen.contains("/device"), "{}", login.seen);
    // The CLI polls while the person has not answered.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(!h.credentials_file().exists());

    h.approve_device(&login.found, "approve").await;
    let ran = login.finish().await;

    assert_eq!(ran.status, Some(0), "{}", ran.stderr);
    assert_eq!(mode(&h.credentials_file()), 0o600);
    assert_no_credentials_in(&ran.stderr, &h.stored());
    let listed = h.bia(&["workflow", "list"]).await;
    assert_eq!(listed.status, Some(0), "{}", listed.stderr);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_denied_device_login_fails_and_stores_nothing(pool: PgPool) {
    let h = start(pool).await;
    let login = h
        .start_login(&["login", "--device"], |line| {
            (line.len() == 9 && line.as_bytes()[4] == b'-').then(|| line.to_owned())
        })
        .await;

    h.approve_device(&login.found, "deny").await;
    let ran = login.finish().await;

    assert_eq!(ran.status, Some(1));
    assert!(ran.stderr.contains("denied"), "{}", ran.stderr);
    assert!(!h.credentials_file().exists());
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn an_expired_access_token_is_refreshed_before_the_request(pool: PgPool) {
    let h = start(pool).await;
    h.log_in().await;
    let before = h.stored();
    h.rewrite_stored(|login| login["expires_at"] = json!("2020-01-01T00:00:00Z"));

    let listed = h.bia(&["workflow", "list"]).await;

    assert_eq!(listed.status, Some(0), "{}", listed.stderr);
    let after = h.stored();
    assert_ne!(after["access_token"], before["access_token"]);
    assert_ne!(after["refresh_token"], before["refresh_token"]);
    assert_eq!(mode(&h.credentials_file()), 0o600);
    assert_no_credentials_in(&listed.stderr, &after);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_rejected_access_token_is_refreshed_and_the_request_retried(pool: PgPool) {
    let h = start(pool).await;
    h.log_in().await;
    let before = h.stored();
    // The server revokes the access token while the Grant lives on.
    let tokens: Value = h
        .as_person(h.http.get(format!("{}/v1/tokens", h.url)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let access = tokens["tokens"]
        .as_array()
        .unwrap()
        .iter()
        .find(|token| token["origin"] == "oauth")
        .unwrap();
    let revoked = h
        .as_person(h.http.post(format!(
            "{}/v1/tokens/{}/revoke",
            h.url,
            access["id"].as_str().unwrap()
        )))
        .send()
        .await
        .unwrap();
    assert_eq!(revoked.status(), 200);

    let listed = h.bia(&["workflow", "list"]).await;

    assert_eq!(listed.status, Some(0), "{}", listed.stderr);
    assert_ne!(h.stored()["access_token"], before["access_token"]);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn a_revoked_grant_fails_with_the_login_hint_and_stops_refreshing(pool: PgPool) {
    let h = start(pool).await;
    h.log_in().await;
    let grants: Value = h
        .as_person(h.http.get(format!("{}/v1/grants", h.url)))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let grant = grants["grants"][0]["id"].as_str().unwrap();
    let revoked = h
        .as_person(h.http.post(format!("{}/v1/grants/{grant}/revoke", h.url)))
        .send()
        .await
        .unwrap();
    assert_eq!(revoked.status(), 200);

    let listed = h.bia(&["workflow", "list"]).await;

    assert_eq!(listed.status, Some(1));
    assert!(
        listed.stderr.contains("hint: run `bia login`"),
        "{}",
        listed.stderr
    );
    // The stored login can never work again, so it is gone.
    assert!(
        !h.credentials_file().exists() || h.stored().is_null(),
        "the dead login is still stored"
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn bia_token_wins_over_a_stored_login(pool: PgPool) {
    let h = start(pool).await;
    h.log_in().await;
    let wrong = format!("bia_{}", "0".repeat(64));

    let with_wrong_token = h.bia_with(Some(&wrong), &["workflow", "list"]).await;

    assert_eq!(with_wrong_token.status, Some(1));
    assert!(
        with_wrong_token.stderr.contains("invalid_token"),
        "{}",
        with_wrong_token.stderr
    );
    // And the stored login was not touched by it.
    let without = h.bia(&["workflow", "list"]).await;
    assert_eq!(without.status, Some(0), "{}", without.stderr);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn logout_revokes_the_grant_and_deletes_the_stored_login(pool: PgPool) {
    let h = start(pool).await;
    h.log_in().await;
    let stored = h.stored();
    let access = stored["access_token"].as_str().unwrap().to_owned();
    let works = h
        .http
        .get(format!("{}/v1/tokens/me", h.url))
        .bearer_auth(&access)
        .send()
        .await
        .unwrap();
    assert_eq!(works.status(), 200);

    let ran = h.bia(&["logout"]).await;

    assert_eq!(ran.status, Some(0), "{}", ran.stderr);
    assert_no_credentials_in(&ran.stderr, &stored);
    assert_no_credentials_in(&ran.stdout, &stored);
    let revoked = h
        .http
        .get(format!("{}/v1/tokens/me", h.url))
        .bearer_auth(&access)
        .send()
        .await
        .unwrap();
    assert_eq!(revoked.status(), 401);
    assert!(h.stored().is_null());
    let after = h.bia(&["workflow", "list"]).await;
    assert_eq!(after.status, Some(1));
    let again = h.bia(&["logout"]).await;
    assert_eq!(again.status, Some(0));
    assert!(again.stderr.contains("Not logged in"), "{}", again.stderr);
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn bia_mcp_works_with_only_a_stored_login(pool: PgPool) {
    let h = start(pool).await;
    h.log_in().await;
    let mut child = h
        .command(&["mcp"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let initialize = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "test", "version": "0" },
        },
    });
    stdin
        .write_all(format!("{initialize}\n").as_bytes())
        .await
        .unwrap();

    let line = tokio::time::timeout(Duration::from_secs(30), lines.next_line())
        .await
        .expect("no reply from bia mcp")
        .unwrap()
        .expect("bia mcp closed stdout");

    let reply: Value = serde_json::from_str(&line).unwrap();
    assert!(
        reply["result"]["capabilities"]["tools"].is_object(),
        "{reply}"
    );
}

#[sqlx::test(migrator = "bia_store::MIGRATOR")]
async fn login_against_a_server_that_does_not_offer_sign_in_says_so(pool: PgPool) {
    let h = start(pool).await;

    let ran = h
        .bia(&["--url", "http://127.0.0.1:1", "login", "--no-browser"])
        .await;

    assert_eq!(ran.status, Some(1));
    assert!(ran.stderr.contains("cannot reach"), "{}", ran.stderr);
    assert!(!h.credentials_file().exists());
}
