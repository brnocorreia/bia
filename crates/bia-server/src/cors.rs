//! Which browser origins may call the API, for a UI hosted apart from it.

use std::time::Duration;

use axum::http::{HeaderValue, Method, header};
use tower_http::cors::{AllowOrigin, CorsLayer};

/// Origins allowed to call the API from a browser. A preflight from any other origin gets no
/// CORS headers, so the browser refuses the request.
#[derive(Debug, Clone, Default)]
pub enum CorsOrigins {
    /// No browser origin; non-browser callers are unaffected.
    #[default]
    None,
    /// `*`: any origin. Tokens travel in a header the page must set, never in cookies, so a
    /// page cannot borrow a caller's credentials.
    Any,
    List(Vec<HeaderValue>),
}

#[derive(Debug, thiserror::Error)]
#[error(
    "`{0}` is not an origin; write it as the browser sends it, e.g. `https://bia.example.com` \
     or `http://localhost:5173`, without a path"
)]
pub struct InvalidOrigin(pub String);

impl CorsOrigins {
    /// Reads each origin, or `*` alone for any.
    pub fn parse<S: AsRef<str>>(
        origins: impl IntoIterator<Item = S>,
    ) -> Result<Self, InvalidOrigin> {
        let origins: Vec<String> = origins
            .into_iter()
            .map(|origin| origin.as_ref().trim().to_owned())
            .filter(|origin| !origin.is_empty())
            .collect();
        if origins.is_empty() {
            return Ok(Self::None);
        }
        if origins.iter().any(|origin| origin == "*") {
            return if origins.len() == 1 {
                Ok(Self::Any)
            } else {
                Err(InvalidOrigin("*".to_owned()))
            };
        }
        origins
            .into_iter()
            .map(|origin| match HeaderValue::from_str(&origin) {
                Ok(value) if is_origin(&origin) => Ok(value),
                _ => Err(InvalidOrigin(origin)),
            })
            .collect::<Result<_, _>>()
            .map(Self::List)
    }

    pub(crate) fn layer(&self) -> CorsLayer {
        let allow = match self {
            Self::None => AllowOrigin::list([]),
            Self::Any => AllowOrigin::any(),
            Self::List(origins) => AllowOrigin::list(origins.iter().cloned()),
        };
        CorsLayer::new()
            .allow_origin(allow)
            .allow_methods([
                Method::GET,
                Method::POST,
                Method::PUT,
                Method::PATCH,
                Method::DELETE,
            ])
            .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
            .max_age(Duration::from_secs(3600))
    }
}

/// `scheme://host[:port]`, lowercase, as a browser's `Origin` header carries it.
fn is_origin(origin: &str) -> bool {
    let Some(authority) = origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("http://"))
    else {
        return false;
    };
    !authority.is_empty()
        && !authority.starts_with(':')
        && authority
            .bytes()
            .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'-' | b':' | b'[' | b']'))
}

#[cfg(test)]
mod tests {
    use super::CorsOrigins;

    #[test]
    fn origins_are_read_as_browsers_send_them() {
        let parsed = CorsOrigins::parse(["https://bia.example.com", " http://localhost:5173 ", ""]);
        assert!(matches!(parsed, Ok(CorsOrigins::List(list)) if list.len() == 2));
        assert!(matches!(CorsOrigins::parse(["*"]), Ok(CorsOrigins::Any)));
        assert!(matches!(
            CorsOrigins::parse(Vec::<String>::new()),
            Ok(CorsOrigins::None)
        ));

        for invalid in [
            "bia.example.com",
            "https://bia.example.com/",
            "https://bia.example.com/ui",
            "HTTPS://BIA.EXAMPLE.COM",
            "ftp://bia.example.com",
            "https://",
        ] {
            assert!(CorsOrigins::parse([invalid]).is_err(), "{invalid}");
        }
        assert!(CorsOrigins::parse(["*", "https://bia.example.com"]).is_err());
    }
}
