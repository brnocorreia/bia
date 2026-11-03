//! Fetching a client's metadata document from a URL its owner chose (ADR-0067).
//!
//! The URL comes from an unauthenticated request, so the fetch is an SSRF vector: it resolves
//! the host itself, refuses the whole answer if any address in it is not public, and connects
//! to the addresses it checked instead of letting the HTTP client resolve the name again, so a
//! DNS answer that changes between check and connection (rebinding) buys nothing. It follows no
//! redirects, reads at most [`MAX_BYTES`] and gives up after [`TIMEOUT`]. No proxy is used, since
//! one would connect on the fetcher's behalf to an address nobody checked.
//!
//! The production fetcher is [`ClientMetadataFetcher::strict`]. The relaxed ones the tests need
//! (a `wiremock` server is plain HTTP on loopback) exist only under the `testing` feature, so
//! no configuration can build one.

use std::fmt;
use std::future::Future;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use url::{Host, Url};

/// A metadata document is a few hundred bytes; this leaves room and no more.
pub(crate) const MAX_BYTES: usize = 5 * 1024;
pub(crate) const TIMEOUT: Duration = Duration::from_secs(5);
/// The longest a document is trusted without asking again, whatever its cache headers say.
pub(crate) const MAX_CACHE: Duration = Duration::from_secs(24 * 60 * 60);

/// Turns a host name into addresses. The server asks the system's resolver; a test supplies its
/// own to stage a rebinding or a private answer.
pub trait Resolve: fmt::Debug + Send + Sync {
    fn resolve<'a>(
        &'a self,
        host: &'a str,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = io::Result<Vec<IpAddr>>> + Send + 'a>>;
}

#[derive(Debug)]
struct SystemResolver;

impl Resolve for SystemResolver {
    fn resolve<'a>(
        &'a self,
        host: &'a str,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = io::Result<Vec<IpAddr>>> + Send + 'a>> {
        Box::pin(async move {
            Ok(tokio::net::lookup_host((host, port))
                .await?
                .map(|address| address.ip())
                .collect())
        })
    }
}

/// Why a document was not fetched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FetchError {
    /// Not HTTPS, or the host is, or resolves to, an address that is not public.
    Blocked,
    /// No answer in time, no such host, a connection failure or a status other than 200.
    Unreachable,
    TooLarge,
    /// The answer is a redirect.
    Redirected,
}

/// A document and how long it may be reused.
#[derive(Debug)]
pub(crate) struct Fetched {
    pub(crate) body: Vec<u8>,
    /// From `Cache-Control: max-age`, at most [`MAX_CACHE`]; `None` when it says nothing, or
    /// says not to store.
    pub(crate) max_age: Option<Duration>,
}

#[derive(Clone)]
pub struct ClientMetadataFetcher {
    resolver: Arc<dyn Resolve>,
    /// Lets plain HTTP and loopback addresses through. Only a test can set it.
    local: bool,
}

impl ClientMetadataFetcher {
    /// What the server uses: HTTPS to public addresses only.
    pub fn strict() -> Self {
        Self {
            resolver: Arc::new(SystemResolver),
            local: false,
        }
    }

    /// Reaches plain HTTP servers on loopback, such as `wiremock`.
    #[cfg(feature = "testing")]
    pub fn allowing_local_http_for_tests() -> Self {
        Self {
            resolver: Arc::new(SystemResolver),
            local: true,
        }
    }

    /// Resolves names with `resolver`; `local` is as for
    /// [`allowing_local_http_for_tests`](Self::allowing_local_http_for_tests).
    #[cfg(feature = "testing")]
    pub fn with_resolver_for_tests(resolver: impl Resolve + 'static, local: bool) -> Self {
        Self {
            resolver: Arc::new(resolver),
            local,
        }
    }

    pub(crate) async fn fetch(&self, url: &Url) -> Result<Fetched, FetchError> {
        let secure = url.scheme() == "https";
        if !secure && !(self.local && url.scheme() == "http") {
            return Err(FetchError::Blocked);
        }
        let port = url.port_or_known_default().ok_or(FetchError::Blocked)?;
        let pinned = match url.host().ok_or(FetchError::Blocked)? {
            Host::Ipv4(ip) => vec![IpAddr::V4(ip)],
            Host::Ipv6(ip) => vec![IpAddr::V6(ip)],
            Host::Domain(name) => self
                .resolver
                .resolve(name, port)
                .await
                .map_err(|_| FetchError::Unreachable)?,
        };
        if pinned.is_empty() {
            return Err(FetchError::Unreachable);
        }
        // One bad address refuses the lot: the client could be handed any of them.
        if !self.local && pinned.iter().any(|ip| is_blocked(*ip)) {
            return Err(FetchError::Blocked);
        }
        let mut client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(TIMEOUT)
            .no_proxy()
            .user_agent("bia");
        if let Host::Domain(name) = url.host().ok_or(FetchError::Blocked)? {
            let addresses: Vec<SocketAddr> =
                pinned.iter().map(|ip| SocketAddr::new(*ip, port)).collect();
            client = client.resolve_to_addrs(name, &addresses);
        }
        let client = client.build().map_err(|_| FetchError::Unreachable)?;
        let mut response = client
            .get(url.as_str())
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|_| FetchError::Unreachable)?;
        let status = response.status();
        if status.is_redirection() {
            return Err(FetchError::Redirected);
        }
        if status != reqwest::StatusCode::OK {
            return Err(FetchError::Unreachable);
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_BYTES as u64)
        {
            return Err(FetchError::TooLarge);
        }
        let max_age = response
            .headers()
            .get(reqwest::header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok())
            .and_then(max_age);
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| FetchError::Unreachable)?
        {
            if body.len() + chunk.len() > MAX_BYTES {
                return Err(FetchError::TooLarge);
            }
            body.extend_from_slice(&chunk);
        }
        Ok(Fetched { body, max_age })
    }
}

impl Default for ClientMetadataFetcher {
    fn default() -> Self {
        Self::strict()
    }
}

impl fmt::Debug for ClientMetadataFetcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientMetadataFetcher")
            .field("local", &self.local)
            .finish_non_exhaustive()
    }
}

/// How long `Cache-Control` lets a document be reused; `None` when it gives no lifetime or
/// forbids storing.
fn max_age(header: &str) -> Option<Duration> {
    let mut seconds = None;
    for directive in header.split(',').map(|d| d.trim().to_ascii_lowercase()) {
        if directive == "no-store" || directive == "no-cache" {
            return None;
        }
        if let Some(value) = directive.strip_prefix("max-age=") {
            seconds = value.trim_matches('"').parse::<u64>().ok();
        }
    }
    seconds
        .filter(|seconds| *seconds > 0)
        .map(|seconds| Duration::from_secs(seconds).min(MAX_CACHE))
}

/// Whether bia refuses to connect to `ip`: anything that is not a public unicast address,
/// including IPv4 addresses written as IPv6 (mapped, compatible, NAT64, 6to4).
pub(crate) fn is_blocked(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => blocked_v4(ip),
        IpAddr::V6(ip) => blocked_v6(ip),
    }
}

fn blocked_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_multicast()
        || a == 0
        // Shared address space (carrier-grade NAT).
        || (a == 100 && (64..128).contains(&b))
        // IETF protocol assignments, and the documentation and benchmarking ranges.
        || (a == 192 && b == 0 && c == 0)
        || (a == 192 && b == 0 && c == 2)
        || (a == 198 && (b == 18 || b == 19))
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
        // Reserved for future use.
        || a >= 240
}

fn blocked_v6(ip: Ipv6Addr) -> bool {
    let segments = ip.segments();
    if ip.is_unspecified() || ip.is_loopback() || ip.is_multicast() {
        return true;
    }
    // IPv4 inside IPv6: judge it as the IPv4 address it carries.
    if let Some(v4) = ip.to_ipv4_mapped() {
        return blocked_v4(v4);
    }
    let embedded = |a: u16, b: u16| Ipv4Addr::new((a >> 8) as u8, a as u8, (b >> 8) as u8, b as u8);
    // ::a.b.c.d (deprecated IPv4-compatible) and 64:ff9b::a.b.c.d (NAT64).
    if segments[..6] == [0; 6] || segments[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        return blocked_v4(embedded(segments[6], segments[7]));
    }
    // 2002::/16 (6to4) carries its IPv4 address in the next 32 bits.
    if segments[0] == 0x2002 {
        return blocked_v4(embedded(segments[1], segments[2]));
    }
    // fe80::/10 link-local, fec0::/10 site-local, fc00::/7 unique local.
    (segments[0] & 0xffc0) == 0xfe80
        || (segments[0] & 0xffc0) == 0xfec0
        || (segments[0] & 0xfe00) == 0xfc00
        // 2001::/32 Teredo, 2001:db8::/32 documentation, 100::/64 discard-only.
        || (segments[0] == 0x2001 && (segments[1] == 0 || segments[1] == 0x0db8))
        || segments[..4] == [0x100, 0, 0, 0]
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::net::IpAddr;

    use super::{ClientMetadataFetcher, FetchError, Resolve, is_blocked, max_age};

    fn blocked(ip: &str) -> bool {
        is_blocked(ip.parse().unwrap())
    }

    #[test]
    fn loopback_private_and_link_local_ipv4_addresses_are_blocked() {
        for ip in [
            "127.0.0.1",
            "127.255.255.254",
            "10.0.0.1",
            "172.16.0.1",
            "172.31.255.255",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "255.255.255.255",
            "224.0.0.1",
            "240.0.0.1",
            "198.18.0.1",
        ] {
            assert!(blocked(ip), "{ip}");
        }
    }

    #[test]
    fn public_addresses_are_not_blocked() {
        for ip in [
            "8.8.8.8",
            "1.1.1.1",
            "172.32.0.1",
            "100.128.0.1",
            "2606:4700:4700::1111",
            "2001:4860:4860::8888",
        ] {
            assert!(!blocked(ip), "{ip}");
        }
    }

    #[test]
    fn ipv6_loopback_unique_local_and_link_local_addresses_are_blocked() {
        for ip in [
            "::1",
            "::",
            "fe80::1",
            "fc00::1",
            "fd12:3456::1",
            "ff02::1",
            "2001:db8::1",
        ] {
            assert!(blocked(ip), "{ip}");
        }
    }

    #[test]
    fn ipv4_written_as_ipv6_is_judged_as_ipv4() {
        for ip in [
            "::ffff:127.0.0.1",
            "::ffff:10.0.0.1",
            "::ffff:169.254.169.254",
            "::ffff:7f00:1",
            "::127.0.0.1",
            "64:ff9b::a00:1",
            "2002:7f00:1::1",
        ] {
            assert!(blocked(ip), "{ip}");
        }
        assert!(!blocked("::ffff:8.8.8.8"));
        assert!(!blocked("64:ff9b::808:808"));
    }

    #[derive(Debug)]
    struct Answers(Vec<IpAddr>);

    impl Resolve for Answers {
        fn resolve<'a>(
            &'a self,
            _: &'a str,
            _: u16,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = std::io::Result<Vec<IpAddr>>> + Send + 'a>,
        > {
            Box::pin(std::future::ready(Ok(self.0.clone())))
        }
    }

    async fn fetch(fetcher: &ClientMetadataFetcher, url: &str) -> Result<(), FetchError> {
        fetcher.fetch(&url.parse().unwrap()).await.map(|_| ())
    }

    #[tokio::test]
    async fn the_strict_fetcher_refuses_plain_http_and_literal_private_addresses() {
        let fetcher = ClientMetadataFetcher::strict();

        for url in [
            "http://example.com/client.json",
            "https://127.0.0.1/client.json",
            "https://[::1]/client.json",
            "https://169.254.169.254/latest/meta-data",
            "https://[::ffff:10.0.0.1]/client.json",
        ] {
            assert_eq!(
                fetch(&fetcher, url).await,
                Err(FetchError::Blocked),
                "{url}"
            );
        }
    }

    #[tokio::test]
    async fn a_name_that_resolves_to_a_private_address_is_refused_without_connecting() {
        let private: IpAddr = "10.1.2.3".parse().unwrap();
        let public: IpAddr = "93.184.216.34".parse().unwrap();

        // Any one private address in the answer refuses it, so a client cannot be handed the
        // private one after the public one passed.
        for answers in [vec![private], vec![public, private], vec![private, public]] {
            let fetcher = ClientMetadataFetcher::with_resolver_for_tests(Answers(answers), false);
            assert_eq!(
                fetch(&fetcher, "https://rebind.example/client.json").await,
                Err(FetchError::Blocked)
            );
        }
    }

    #[tokio::test]
    async fn a_name_that_resolves_to_nothing_is_unreachable() {
        let fetcher = ClientMetadataFetcher::with_resolver_for_tests(Answers(vec![]), false);

        assert_eq!(
            fetch(&fetcher, "https://nowhere.example/client.json").await,
            Err(FetchError::Unreachable)
        );
    }

    #[test]
    fn cache_headers_set_the_lifetime_up_to_a_day() {
        let secs = |header| max_age(header).map(|d| d.as_secs());

        assert_eq!(secs("max-age=60"), Some(60));
        assert_eq!(secs("public, max-age=3600"), Some(3600));
        assert_eq!(secs("max-age=999999999"), Some(86_400));
        assert_eq!(secs("no-store, max-age=60"), None);
        assert_eq!(secs("max-age=0"), None);
        assert_eq!(secs("public"), None);
    }
}
