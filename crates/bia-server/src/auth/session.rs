//! The Session cookie and the Token row behind it (ADR-0064).

use axum::http::{HeaderMap, HeaderValue, header};
use bia_store::token::{
    NewToken, Origin, PgTokenRepository, SESSION_MAX_LIFETIME, Scope, Token, TokenRepository,
};
use bia_store::user::UserId;
use sqlx::PgConnection;

use super::TokenValue;
use crate::{ApiError, ApiState};

pub(crate) const COOKIE: &str = "bia_session";

/// The Token value in the Session cookie, if the request carries one.
pub(crate) fn cookie_value(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .find_map(|pair| {
            let (name, value) = pair.split_once('=')?;
            (name.trim() == COOKIE).then(|| value.trim())
        })
}

/// A `Set-Cookie` that starts the Session. No `Domain`, so only the host that set it gets it
/// back; `Max-Age` is the longest a Session can last, and the server decides when it ends.
pub(crate) fn set_cookie(value: &TokenValue, secure: bool) -> HeaderValue {
    cookie(value.as_str(), SESSION_MAX_LIFETIME.num_seconds(), secure)
}

/// A `Set-Cookie` that makes the browser drop the cookie.
pub(crate) fn clear_cookie(secure: bool) -> HeaderValue {
    cookie("", 0, secure)
}

fn cookie(value: &str, max_age: i64, secure: bool) -> HeaderValue {
    let secure = if secure { "; Secure" } else { "" };
    HeaderValue::from_str(&format!(
        "{COOKIE}={value}; Max-Age={max_age}; Path=/; HttpOnly; SameSite=Lax{secure}"
    ))
    // The value is hex and the rest is fixed text.
    .unwrap_or_else(|_| unreachable!("a cookie is visible ASCII"))
}

/// Starts an `admin` Session for the User.
pub(crate) async fn start(
    state: &ApiState,
    conn: &mut PgConnection,
    user_id: UserId,
) -> Result<(Token, TokenValue), ApiError> {
    let (value, hash) = TokenValue::generate().map_err(|e| ApiError::internal(&e))?;
    let token = PgTokenRepository
        .create(
            conn,
            &NewToken {
                name: "session".to_owned(),
                hash,
                scope: Scope::Admin,
                origin: Origin::Session,
                user_id,
                created_at: state.clock.now(),
            },
        )
        .await
        .map_err(|e| ApiError::internal(&e))?;
    Ok((token, value))
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue, header};

    use super::{clear_cookie, cookie_value};

    #[test]
    fn the_session_cookie_is_found_among_others() {
        let mut headers = HeaderMap::new();
        headers.append(
            header::COOKIE,
            HeaderValue::from_static("a=1; bia_session=abc"),
        );
        headers.append(header::COOKIE, HeaderValue::from_static("b=2"));

        assert_eq!(cookie_value(&headers), Some("abc"));
        assert_eq!(cookie_value(&HeaderMap::new()), None);
    }

    #[test]
    fn clearing_the_cookie_expires_it_at_once() {
        let cleared = clear_cookie(true);

        assert_eq!(
            cleared,
            "bia_session=; Max-Age=0; Path=/; HttpOnly; SameSite=Lax; Secure"
        );
    }
}
