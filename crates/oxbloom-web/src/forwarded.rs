//! Client address, scheme and host of a request, resolved from trusted forwarded headers.
//!
//! Deployment requirement: the framework trusts what a trusted proxy passes on and cannot tell
//! which values the proxy wrote itself. The trusted proxy must set or strip `Forwarded`, must
//! overwrite or strip `X-Forwarded-Proto` and `X-Forwarded-Host`, and must append the peer it
//! saw to `X-Forwarded-For`. A proxy that passes a client's `Forwarded`, `X-Forwarded-Proto` or
//! `X-Forwarded-Host` through unchanged lets that client choose its address, scheme and host.

use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use http::HeaderMap;

/// Scheme the client used to reach the server (or the first trusted proxy).
///
/// # Examples
///
/// ```
/// use oxbloom_web::{HttpMethod, Request, Scheme};
///
/// assert_eq!(Scheme::default(), Scheme::Http);
/// // A request that did not come through `serve` has no client info: the scheme is `Http`.
/// assert_eq!(Request::new(HttpMethod::Get, "/").scheme(), Scheme::Http);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Scheme {
    /// Plain HTTP; the default.
    #[default]
    Http,
    /// HTTP over TLS, as reported by a trusted proxy.
    Https,
}

/// Where a request came from. [`App::serve`](crate::App::serve) stores one in the extensions of
/// every request; [`Request::client_addr`](crate::Request::client_addr),
/// [`Request::scheme`](crate::Request::scheme) and [`Request::host`](crate::Request::host) read it.
///
/// Without trusted forwarded headers `addr` is the TCP peer, `scheme` is `Http` and `host` is
/// `None`. A forwarded address without a port has port `0`.
///
/// # Examples
///
/// ```
/// use oxbloom_web::{ClientInfo, HttpMethod, Request, Scheme};
///
/// let mut req = Request::new(HttpMethod::Get, "/");
/// req.extensions_mut().insert(ClientInfo {
///     addr: "203.0.113.9:0".parse().unwrap(),
///     scheme: Scheme::Https,
///     host: Some("example.com".into()),
/// });
/// assert_eq!(req.client_addr(), Some("203.0.113.9:0".parse().unwrap()));
/// assert_eq!(req.scheme(), Scheme::Https);
/// assert_eq!(req.host(), Some("example.com"));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientInfo {
    /// Client address: the TCP peer, or the client named by a trusted proxy.
    pub addr: SocketAddr,
    /// Scheme reported by a trusted proxy, else `Http`.
    pub scheme: Scheme,
    /// Host (authority) reported by a trusted proxy, else `None`.
    pub host: Option<String>,
}

/// Whether forwarded headers (`Forwarded`, `X-Forwarded-*`) are honored.
///
/// The enum is `#[non_exhaustive]`: a `match` outside `oxbloom-web` needs a wildcard arm.
///
/// # Examples
///
/// ```
/// use oxbloom_web::ForwardStrategy;
///
/// assert_eq!(ForwardStrategy::default(), ForwardStrategy::None);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum ForwardStrategy {
    /// Ignore every forwarded header; the client is the TCP peer. The default.
    #[default]
    None,
    /// Honor forwarded headers sent by a peer in the trusted list.
    ///
    /// Every trusted proxy must set or strip `Forwarded`, overwrite or strip
    /// `X-Forwarded-Proto` and `X-Forwarded-Host`, and append to `X-Forwarded-For`; otherwise
    /// clients control those values. The framework cannot detect a proxy that does not.
    Framework,
}

/// A list of trusted proxy addresses and networks (`IP` or `IP/prefix`, IPv4 and IPv6).
///
/// IPv4-mapped IPv6 addresses (`::ffff:a.b.c.d`) are matched as IPv4.
///
/// # Examples
///
/// ```
/// use oxbloom_web::TrustedProxies;
///
/// let trusted = TrustedProxies::parse(&["10.0.0.0/8".into(), "::1".into()])?;
/// assert!(trusted.contains("10.1.2.3".parse().unwrap()));
/// assert!(trusted.contains("::ffff:10.1.2.3".parse().unwrap()));
/// assert!(!trusted.contains("192.0.2.1".parse().unwrap()));
/// # Ok::<(), oxbloom_web::ProxyParseError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedProxies {
    nets: Vec<(IpAddr, u8)>,
}

/// A trusted-proxy entry that is not an IP address or CIDR network.
///
/// It names the index of the bad entry, never its text.
///
/// # Examples
///
/// ```
/// use oxbloom_web::TrustedProxies;
///
/// let err = TrustedProxies::parse(&["10.0.0.1".into(), "10.0.0.0/33".into()]).unwrap_err();
/// assert_eq!(err.index(), 1);
/// assert!(!err.to_string().contains("10.0.0.0"));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyParseError {
    index: usize,
}

/// Forwarded-header handling of an [`App`](crate::App): the strategy and the trusted peers.
///
/// The default ignores forwarded headers and lists loopback as trusted (used only once the
/// strategy is `Framework`).
///
/// # Examples
///
/// ```
/// use oxbloom_web::{App, ForwardStrategy, ForwardedConfig, TrustedProxies};
///
/// let cfg = ForwardedConfig {
///     strategy: ForwardStrategy::Framework,
///     trusted: TrustedProxies::parse(&["10.0.0.0/8".into()])?,
/// };
/// let app = App::new().forwarded(cfg);
/// # let _ = app;
/// # Ok::<(), oxbloom_web::ProxyParseError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardedConfig {
    /// Whether forwarded headers are honored.
    pub strategy: ForwardStrategy,
    /// Peers whose forwarded headers are trusted.
    pub trusted: TrustedProxies,
}

impl Default for ForwardedConfig {
    fn default() -> Self {
        Self {
            strategy: ForwardStrategy::None,
            trusted: TrustedProxies::loopback(),
        }
    }
}

impl TrustedProxies {
    /// Parses entries of the form `IP` or `IP/prefix` (IPv4 prefix `0..=32`, IPv6 `0..=128`).
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::TrustedProxies;
    ///
    /// let trusted = TrustedProxies::parse(&["192.168.1.5".into(), "fd00::/8".into()])?;
    /// assert!(trusted.contains("192.168.1.5".parse().unwrap()));
    /// assert!(trusted.contains("fd12::1".parse().unwrap()));
    /// assert!(TrustedProxies::parse(&["not-an-ip".into()]).is_err());
    /// # Ok::<(), oxbloom_web::ProxyParseError>(())
    /// ```
    pub fn parse(entries: &[String]) -> Result<Self, ProxyParseError> {
        let nets = entries
            .iter()
            .enumerate()
            .map(|(index, e)| parse_net(e).ok_or(ProxyParseError { index }))
            .collect::<Result<_, _>>()?;
        Ok(Self { nets })
    }

    /// `127.0.0.0/8` and `::1`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::TrustedProxies;
    ///
    /// let trusted = TrustedProxies::loopback();
    /// assert!(trusted.contains("127.0.0.1".parse().unwrap()));
    /// assert!(trusted.contains("::1".parse().unwrap()));
    /// assert!(!trusted.contains("10.0.0.1".parse().unwrap()));
    /// ```
    pub fn loopback() -> Self {
        Self {
            nets: vec![
                (IpAddr::V4(Ipv4Addr::new(127, 0, 0, 0)), 8),
                (IpAddr::V6(Ipv6Addr::LOCALHOST), 128),
            ],
        }
    }

    /// Whether `ip` is in one of the trusted networks.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::TrustedProxies;
    ///
    /// let trusted = TrustedProxies::parse(&["0.0.0.0/0".into()])?;
    /// assert!(trusted.contains("198.51.100.7".parse().unwrap()));
    /// assert!(!trusted.contains("2001:db8::1".parse().unwrap()));
    /// # Ok::<(), oxbloom_web::ProxyParseError>(())
    /// ```
    pub fn contains(&self, ip: IpAddr) -> bool {
        let ip = ip.to_canonical();
        self.nets.iter().any(|&(net, prefix)| match (net, ip) {
            (IpAddr::V4(n), IpAddr::V4(a)) => {
                let mask = mask_u32(prefix);
                u32::from(n) & mask == u32::from(a) & mask
            }
            (IpAddr::V6(n), IpAddr::V6(a)) => {
                let mask = mask_u128(prefix);
                u128::from(n) & mask == u128::from(a) & mask
            }
            _ => false,
        })
    }
}

impl ProxyParseError {
    /// Zero-based index of the entry that failed to parse.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_web::TrustedProxies;
    ///
    /// let err = TrustedProxies::parse(&["x".into()]).unwrap_err();
    /// assert_eq!(err.index(), 0);
    /// ```
    pub fn index(&self) -> usize {
        self.index
    }
}

impl fmt::Display for ProxyParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "trusted proxy entry {} is not an IP address or CIDR network",
            self.index
        )
    }
}

impl std::error::Error for ProxyParseError {}

fn mask_u32(prefix: u8) -> u32 {
    u32::MAX.checked_shl(32 - u32::from(prefix)).unwrap_or(0)
}

fn mask_u128(prefix: u8) -> u128 {
    u128::MAX.checked_shl(128 - u32::from(prefix)).unwrap_or(0)
}

/// `IP` or `IP/prefix`; an IPv4-mapped IPv6 network with a prefix of at least 96 becomes IPv4.
fn parse_net(entry: &str) -> Option<(IpAddr, u8)> {
    let entry = entry.trim();
    let (addr, prefix) = match entry.split_once('/') {
        Some((a, p)) => {
            if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            (a, Some(p.parse::<u8>().ok()?))
        }
        None => (entry, None),
    };
    let ip: IpAddr = addr.parse().ok()?;
    let max = if ip.is_ipv4() { 32 } else { 128 };
    let prefix = prefix.unwrap_or(max);
    if prefix > max {
        return None;
    }
    if let IpAddr::V6(v6) = ip
        && let Some(v4) = v6.to_ipv4_mapped()
        && prefix >= 96
    {
        return Some((IpAddr::V4(v4), prefix - 96));
    }
    Some((ip, prefix))
}

/// Resolves the client of one request from the TCP `peer` and, when allowed, its headers.
///
/// Forwarded headers are plain request headers: anyone can send them. They describe the client
/// only when a proxy we trust wrote them, so they are read only when the strategy is
/// `Framework` and the TCP peer (the one address no client can forge) is a trusted proxy.
/// The same holds for `proto` and `host`: an untrusted client could otherwise claim `https` or
/// any host and steer redirects, absolute links or security decisions built on them.
pub(crate) fn resolve_client(
    peer: SocketAddr,
    headers: &HeaderMap,
    cfg: &ForwardedConfig,
) -> ClientInfo {
    let fallback = ClientInfo {
        addr: peer,
        scheme: Scheme::Http,
        host: None,
    };
    if cfg.strategy != ForwardStrategy::Framework || !cfg.trusted.contains(peer.ip()) {
        return fallback;
    }
    let resolved = if headers.contains_key(http::header::FORWARDED) {
        from_forwarded(headers, &cfg.trusted)
    } else {
        from_x_forwarded(headers, &cfg.trusted, peer)
    };
    resolved.unwrap_or(fallback)
}

/// All lines of header `name` joined with `,`, or `None` when one is not visible ASCII.
fn joined(headers: &HeaderMap, name: &str) -> Option<Option<String>> {
    let mut out: Option<String> = None;
    for v in headers.get_all(name) {
        let v = v.to_str().ok()?;
        match &mut out {
            Some(s) => {
                s.push(',');
                s.push_str(v);
            }
            None => out = Some(v.to_owned()),
        }
    }
    Some(out)
}

/// Index of the client among `hops`: walking from the right, the first untrusted hop; when
/// every hop is trusted, the leftmost.
fn client_index(hops: &[SocketAddr], trusted: &TrustedProxies) -> usize {
    hops.iter()
        .rposition(|h| !trusted.contains(h.ip()))
        .unwrap_or(0)
}

fn from_x_forwarded(
    headers: &HeaderMap,
    trusted: &TrustedProxies,
    peer: SocketAddr,
) -> Option<ClientInfo> {
    let hops = match joined(headers, "x-forwarded-for")? {
        Some(list) => list.split(',').map(parse_hop).collect::<Option<Vec<_>>>()?,
        None => Vec::new(),
    };
    let (addr, index) = if hops.is_empty() {
        (peer, None)
    } else {
        let i = client_index(&hops, trusted);
        (hops[i], Some(i))
    };
    // A list in `-Proto`/`-Host` lines up with `-For` when the lengths match; otherwise the
    // rightmost value, written by the proxy nearest the server, is used.
    let pick = |name: &str| -> Option<String> {
        let list = joined(headers, name)??;
        let values: Vec<&str> = list.split(',').map(str::trim).collect();
        let v = match index {
            Some(i) if values.len() == hops.len() => values[i],
            _ => values.last().copied()?,
        };
        Some(v.to_owned())
    };
    Some(ClientInfo {
        addr,
        scheme: pick("x-forwarded-proto").map_or(Scheme::Http, |p| parse_proto(&p)),
        host: pick("x-forwarded-host").filter(|h| is_authority(h)),
    })
}

/// One `Forwarded` element (RFC 7239): its `for`, `proto` and `host` values, unquoted.
#[derive(Default)]
struct Element {
    r#for: Option<String>,
    proto: Option<String>,
    host: Option<String>,
}

fn from_forwarded(headers: &HeaderMap, trusted: &TrustedProxies) -> Option<ClientInfo> {
    let list = joined(headers, "forwarded")??;
    let elements = split_quoted(&list, b',')?
        .into_iter()
        .map(parse_element)
        .collect::<Option<Vec<_>>>()?;
    let hops = elements
        .iter()
        .map(|e| parse_hop(e.r#for.as_deref()?))
        .collect::<Option<Vec<_>>>()?;
    let i = client_index(&hops, trusted);
    let el = &elements[i];
    Some(ClientInfo {
        addr: hops[i],
        scheme: el.proto.as_deref().map_or(Scheme::Http, parse_proto),
        host: el.host.clone().filter(|h| is_authority(h)),
    })
}

/// Splits `s` at `sep` outside double-quoted strings; `None` on an unterminated quote.
fn split_quoted(s: &str, sep: u8) -> Option<Vec<&str>> {
    let mut parts = Vec::new();
    let (mut start, mut quoted, mut escaped) = (0, false, false);
    for (i, b) in s.bytes().enumerate() {
        if escaped {
            escaped = false;
        } else if quoted && b == b'\\' {
            escaped = true;
        } else if b == b'"' {
            quoted = !quoted;
        } else if !quoted && b == sep {
            parts.push(&s[start..i]);
            start = i + 1;
        }
    }
    if quoted {
        return None;
    }
    parts.push(&s[start..]);
    Some(parts)
}

/// `key=value` pairs separated by `;`. Every pair must be well formed, no parameter may repeat,
/// and the element must have a `for` value (an element without one names no hop).
fn parse_element(element: &str) -> Option<Element> {
    let mut el = Element::default();
    for pair in split_quoted(element, b';')? {
        let (key, value) = pair.trim().split_once('=')?;
        let value = unquote(value.trim())?;
        let slot = match key.trim().to_ascii_lowercase().as_str() {
            "for" => &mut el.r#for,
            "proto" => &mut el.proto,
            "host" => &mut el.host,
            "by" => continue,
            _ => return None,
        };
        if slot.replace(value).is_some() {
            return None;
        }
    }
    el.r#for.is_some().then_some(el)
}

/// A token as is, or a quoted string without its quotes and escapes.
fn unquote(v: &str) -> Option<String> {
    if let Some(inner) = v.strip_prefix('"') {
        let inner = inner.strip_suffix('"')?;
        let mut out = String::with_capacity(inner.len());
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            match c {
                '\\' => out.push(chars.next()?),
                '"' => return None,
                c => out.push(c),
            }
        }
        Some(out)
    } else if v.is_empty() || v.contains('"') {
        None
    } else {
        Some(v.to_owned())
    }
}

/// One hop: an IP address, optionally with a port, IPv6 optionally in brackets. Anything else
/// (`unknown`, `_hidden`, a zone id, a bad port) is `None`. A hop without a port gets port 0.
fn parse_hop(hop: &str) -> Option<SocketAddr> {
    let hop = hop.trim();
    if let Ok(ip) = hop.parse::<IpAddr>() {
        return Some(SocketAddr::new(ip, 0));
    }
    if let Ok(sa) = hop.parse::<SocketAddr>() {
        // A scoped IPv6 address (`[fe80::1%2]:80`) is rejected like `fe80::1%eth0`: a zone
        // only means something on the proxy's host.
        if let SocketAddr::V6(v6) = sa
            && v6.scope_id() != 0
        {
            return None;
        }
        return Some(SocketAddr::new(sa.ip(), sa.port()));
    }
    let inner = hop.strip_prefix('[')?.strip_suffix(']')?;
    Some(SocketAddr::new(IpAddr::V6(inner.parse().ok()?), 0))
}

fn parse_proto(p: &str) -> Scheme {
    if p.trim().eq_ignore_ascii_case("https") {
        Scheme::Https
    } else {
        Scheme::Http
    }
}

/// A host with an optional port: a non-empty reg-name or IPv4 address (letters, digits and
/// `-._~%`), or a bracketed IPv6 address, then optionally `:` and a `u16` port. That rules out
/// spaces, control characters, `/`, `@`, `?`, `#`, `\`, quotes, list separators, an empty host
/// or port, and anything after `]` but a port.
fn is_authority(h: &str) -> bool {
    if h.is_empty() || h.len() > 255 {
        return false;
    }
    let (host_ok, port) = if let Some(rest) = h.strip_prefix('[') {
        let Some((inner, after)) = rest.split_once(']') else {
            return false;
        };
        let port = match after {
            "" => None,
            p => match p.strip_prefix(':') {
                Some(p) => Some(p),
                None => return false,
            },
        };
        (inner.parse::<Ipv6Addr>().is_ok(), port)
    } else {
        let (host, port) = match h.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (h, None),
        };
        let ok = !host.is_empty()
            && host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-._~%".contains(&b));
        (ok, port)
    };
    let port_ok = port.is_none_or(|p| {
        !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) && p.parse::<u16>().is_ok()
    });
    host_ok && port_ok
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderValue;

    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn sa(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(
                http::header::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    /// Strategy `Framework`, trusting only `trusted`.
    fn framework(trusted: &[&str]) -> ForwardedConfig {
        ForwardedConfig {
            strategy: ForwardStrategy::Framework,
            trusted: TrustedProxies::parse(&strs(trusted)).unwrap(),
        }
    }

    const PEER: &str = "10.0.0.1:5555";

    fn resolve(cfg: &ForwardedConfig, pairs: &[(&str, &str)]) -> ClientInfo {
        resolve_client(sa(PEER), &headers(pairs), cfg)
    }

    fn peer_only() -> ClientInfo {
        ClientInfo {
            addr: sa(PEER),
            scheme: Scheme::Http,
            host: None,
        }
    }

    #[test]
    fn cidr_parsing_and_matching() {
        let t = TrustedProxies::parse(&strs(&["10.0.0.0/8", "192.168.1.5", "::1", "fd00::/8"]))
            .unwrap();
        assert!(t.contains(ip("10.255.0.1")));
        assert!(!t.contains(ip("11.0.0.1")));
        assert!(t.contains(ip("192.168.1.5")));
        assert!(!t.contains(ip("192.168.1.6")));
        assert!(t.contains(ip("::1")));
        assert!(!t.contains(ip("::2")));
        assert!(t.contains(ip("fd12:3456::1")));
        assert!(!t.contains(ip("fe80::1")));
        // IPv4-mapped IPv6 peers are matched as IPv4.
        assert!(t.contains(ip("::ffff:10.1.2.3")));
        assert!(t.contains(ip("::ffff:192.168.1.5")));

        let all4 = TrustedProxies::parse(&strs(&["0.0.0.0/0"])).unwrap();
        assert!(all4.contains(ip("1.2.3.4")));
        assert!(all4.contains(ip("255.255.255.255")));
        assert!(!all4.contains(ip("::1")));
        let all6 = TrustedProxies::parse(&strs(&["::/0"])).unwrap();
        assert!(all6.contains(ip("2001:db8::1")));
        assert!(!all6.contains(ip("1.2.3.4")));

        let one4 = TrustedProxies::parse(&strs(&["192.0.2.7/32"])).unwrap();
        assert!(one4.contains(ip("192.0.2.7")));
        assert!(!one4.contains(ip("192.0.2.6")));
        let one6 = TrustedProxies::parse(&strs(&["2001:db8::7/128"])).unwrap();
        assert!(one6.contains(ip("2001:db8::7")));
        assert!(!one6.contains(ip("2001:db8::6")));

        // Host bits beyond the prefix are ignored.
        let t = TrustedProxies::parse(&strs(&["10.9.9.9/8"])).unwrap();
        assert!(t.contains(ip("10.0.0.1")));

        for (bad, index) in [
            (vec!["10.0.0.0/33"], 0),
            (vec!["::/129"], 0),
            (vec!["10.0.0.1", "x"], 1),
            (vec![""], 0),
            (vec!["10.0.0.0/"], 0),
            (vec!["10.0.0.0/-1"], 0),
            (vec!["10.0.0.0/8/8"], 0),
            (vec!["/8"], 0),
            (vec!["::1", "::2", "10.0.0.0/+8"], 2),
        ] {
            let err = TrustedProxies::parse(&strs(&bad)).unwrap_err();
            assert_eq!(err.index(), index, "{bad:?}");
            let shown = err.to_string();
            assert!(shown.contains(&index.to_string()), "{shown}");
            // One-letter entries (`x`) occur in any English sentence; check the longer ones.
            if bad[index].len() > 1 {
                assert!(!shown.contains(bad[index]), "echoes the entry: {shown}");
            }
        }

        let lo = TrustedProxies::loopback();
        assert!(lo.contains(ip("127.0.0.1")));
        assert!(lo.contains(ip("127.255.255.254")));
        assert!(lo.contains(ip("::1")));
        assert!(lo.contains(ip("::ffff:127.0.0.1")));
        assert!(!lo.contains(ip("10.0.0.1")));

        let d = ForwardedConfig::default();
        assert_eq!(d.strategy, ForwardStrategy::None);
        assert_eq!(d.trusted, TrustedProxies::loopback());
    }

    #[test]
    fn strategy_none_ignores_headers() {
        let cfg = ForwardedConfig {
            strategy: ForwardStrategy::None,
            trusted: TrustedProxies::parse(&strs(&["0.0.0.0/0", "::/0"])).unwrap(),
        };
        let got = resolve(
            &cfg,
            &[
                ("x-forwarded-for", "203.0.113.9"),
                ("x-forwarded-proto", "https"),
                ("x-forwarded-host", "example.com"),
                ("forwarded", "for=198.51.100.1;proto=https;host=example.org"),
            ],
        );
        assert_eq!(got, peer_only());
    }

    #[test]
    fn untrusted_peer_headers_are_ignored() {
        let cfg = framework(&["192.0.2.0/24"]);
        let got = resolve(
            &cfg,
            &[
                ("x-forwarded-for", "203.0.113.9"),
                ("x-forwarded-proto", "https"),
                ("x-forwarded-host", "example.com"),
            ],
        );
        assert_eq!(got, peer_only());
        let got = resolve(&cfg, &[("forwarded", "for=203.0.113.9;proto=https")]);
        assert_eq!(got, peer_only());
    }

    #[test]
    fn trusted_peer_single_hop_xff() {
        let got = resolve(
            &framework(&["10.0.0.1"]),
            &[
                ("x-forwarded-for", "203.0.113.9"),
                ("x-forwarded-proto", "https"),
                ("x-forwarded-host", "example.com"),
            ],
        );
        assert_eq!(
            got,
            ClientInfo {
                addr: sa("203.0.113.9:0"),
                scheme: Scheme::Https,
                host: Some("example.com".into()),
            }
        );
    }

    #[test]
    fn multi_hop_xff_stops_at_first_untrusted_from_the_right() {
        let cfg = framework(&["10.0.0.0/8"]);
        let got = resolve(
            &cfg,
            &[(
                "x-forwarded-for",
                "198.51.100.1, 203.0.113.9, 10.0.0.7, 10.0.0.8",
            )],
        );
        assert_eq!(got.addr, sa("203.0.113.9:0"));
        // Several header lines are joined in order.
        let got = resolve(
            &cfg,
            &[
                ("x-forwarded-for", "198.51.100.1, 203.0.113.9"),
                ("x-forwarded-for", "10.0.0.7"),
            ],
        );
        assert_eq!(got.addr, sa("203.0.113.9:0"));
        let got = resolve(
            &cfg,
            &[
                ("x-forwarded-for", "10.0.0.7"),
                ("x-forwarded-for", "203.0.113.9"),
            ],
        );
        assert_eq!(got.addr, sa("203.0.113.9:0"));
    }

    #[test]
    fn spoofed_leftmost_entry_is_not_the_client() {
        let got = resolve(
            &framework(&["10.0.0.1"]),
            &[("x-forwarded-for", "6.6.6.6, 203.0.113.9")],
        );
        assert_eq!(got.addr, sa("203.0.113.9:0"));
        let got = resolve(
            &framework(&["10.0.0.1"]),
            &[("forwarded", "for=6.6.6.6, for=203.0.113.9")],
        );
        assert_eq!(got.addr, sa("203.0.113.9:0"));
    }

    #[test]
    fn all_hops_trusted_uses_leftmost() {
        let got = resolve(
            &framework(&["10.0.0.0/8"]),
            &[("x-forwarded-for", "10.0.0.5, 10.0.0.6, 10.0.0.7")],
        );
        assert_eq!(got.addr, sa("10.0.0.5:0"));
    }

    #[test]
    fn ipv6_and_ports_and_brackets() {
        let cfg = framework(&["10.0.0.1"]);
        for (value, want) in [
            ("2001:db8::1", "[2001:db8::1]:0"),
            ("[2001:db8::1]", "[2001:db8::1]:0"),
            ("[2001:db8::1]:8080", "[2001:db8::1]:8080"),
            ("203.0.113.9:4711", "203.0.113.9:4711"),
            ("  203.0.113.9  ", "203.0.113.9:0"),
        ] {
            let got = resolve(&cfg, &[("x-forwarded-for", value)]);
            assert_eq!(got.addr, sa(want), "{value}");
        }
        for (value, want) in [
            ("for=\"[2001:db8::1]:8080\"", "[2001:db8::1]:8080"),
            ("for=\"[2001:db8::1]\"", "[2001:db8::1]:0"),
            ("for=\"203.0.113.9:4711\"", "203.0.113.9:4711"),
            ("For=203.0.113.9", "203.0.113.9:0"),
        ] {
            let got = resolve(&cfg, &[("forwarded", value)]);
            assert_eq!(got.addr, sa(want), "{value}");
        }
        // A scoped IPv6 hop is not usable, in either header.
        for (name, value) in [
            ("x-forwarded-for", "[fe80::1%2]:80"),
            ("forwarded", "for=\"[fe80::1%2]:80\""),
        ] {
            assert_eq!(resolve(&cfg, &[(name, value)]), peer_only(), "{value}");
        }
        // A peer seen as IPv4-mapped IPv6 matches an IPv4 trusted entry.
        let got = resolve_client(
            sa("[::ffff:10.0.0.1]:5555"),
            &headers(&[("x-forwarded-for", "203.0.113.9")]),
            &cfg,
        );
        assert_eq!(got.addr, sa("203.0.113.9:0"));
    }

    #[test]
    fn malformed_value_falls_back_to_peer() {
        let cfg = framework(&["10.0.0.1"]);
        for value in [
            "203.0.113.9, garbage",
            "garbage, 203.0.113.9",
            "203.0.113.9,",
            ",203.0.113.9",
            "203.0.113.9,,198.51.100.1",
            "",
            "unknown",
            "203.0.113.9:99999",
            "[203.0.113.9]",
            "2001:db8::1:8080:x",
            "fe80::1%eth0",
            "[fe80::1%2]:80",
            "[fe80::1%2]",
            "fe80::1%2",
            "203.0.113.9 198.51.100.1",
        ] {
            let got = resolve(
                &cfg,
                &[
                    ("x-forwarded-for", value),
                    ("x-forwarded-proto", "https"),
                    ("x-forwarded-host", "example.com"),
                ],
            );
            assert_eq!(got, peer_only(), "{value:?}");
        }
        for value in [
            "for=unknown",
            "for=_hidden",
            "for=\"_hidden\"",
            "for=203.0.113.9, for=unknown",
            "for=\"203.0.113.9",
            "for=",
            "proto=https",
            "for=203.0.113.9;for=198.51.100.1",
            "for=203.0.113.9;;proto=https",
            "for=203.0.113.9;garbage",
            "for=203.0.113.9, ",
        ] {
            let got = resolve(&cfg, &[("forwarded", value)]);
            assert_eq!(got, peer_only(), "{value:?}");
        }
    }

    #[test]
    fn forwarded_header_takes_precedence_over_x_forwarded() {
        let cfg = framework(&["10.0.0.0/8"]);
        let got = resolve(
            &cfg,
            &[
                ("x-forwarded-for", "198.51.100.1"),
                ("x-forwarded-proto", "http"),
                ("x-forwarded-host", "x.example"),
                (
                    "forwarded",
                    "for=203.0.113.9;proto=https;host=example.com, for=10.0.0.7;proto=http;host=inner",
                ),
            ],
        );
        assert_eq!(
            got,
            ClientInfo {
                addr: sa("203.0.113.9:0"),
                scheme: Scheme::Https,
                host: Some("example.com".into()),
            }
        );
        // A present but unusable `Forwarded` header does not fall back to `X-Forwarded-*`.
        let got = resolve(
            &cfg,
            &[
                ("x-forwarded-for", "198.51.100.1"),
                ("forwarded", "for=unknown"),
            ],
        );
        assert_eq!(got, peer_only());
        // Several `Forwarded` lines are one list; proto/host come from the client's element.
        let got = resolve(
            &cfg,
            &[
                ("forwarded", "for=6.6.6.6;proto=https;host=spoof.example"),
                ("forwarded", "for=203.0.113.9"),
            ],
        );
        assert_eq!(
            got,
            ClientInfo {
                addr: sa("203.0.113.9:0"),
                scheme: Scheme::Http,
                host: None,
            }
        );
        // Quoted values may hold separators.
        let got = resolve(
            &cfg,
            &[(
                "forwarded",
                "for=203.0.113.9;host=\"example.com:8443\";proto=\"HTTPS\"",
            )],
        );
        assert_eq!(got.host.as_deref(), Some("example.com:8443"));
        assert_eq!(got.scheme, Scheme::Https);
        let got = resolve(&cfg, &[("forwarded", "for=203.0.113.9;host=\"a,b;c\"")]);
        assert_eq!(got.addr, sa("203.0.113.9:0"));
        assert_eq!(got.host, None);
    }

    #[test]
    fn proto_accepts_http_https_only() {
        let cfg = framework(&["10.0.0.1"]);
        for (value, want) in [
            ("https", Scheme::Https),
            ("HTTPS", Scheme::Https),
            ("http", Scheme::Http),
            ("ftp", Scheme::Http),
            ("javascript", Scheme::Http),
            ("https ", Scheme::Https),
            ("", Scheme::Http),
        ] {
            let got = resolve(
                &cfg,
                &[
                    ("x-forwarded-for", "203.0.113.9"),
                    ("x-forwarded-proto", value),
                ],
            );
            assert_eq!(got.scheme, want, "{value:?}");
            let got = resolve(&cfg, &[("x-forwarded-proto", value)]);
            assert_eq!(got.scheme, want, "{value:?}");
            assert_eq!(got.addr, sa(PEER));
        }
        let got = resolve(&cfg, &[("forwarded", "for=203.0.113.9;proto=gopher")]);
        assert_eq!(got.scheme, Scheme::Http);
        assert_eq!(got.addr, sa("203.0.113.9:0"));
    }

    #[test]
    fn host_is_taken_when_a_valid_authority() {
        let cfg = framework(&["10.0.0.1"]);
        for (value, want) in [
            ("example.com", Some("example.com")),
            ("example.com:8443", Some("example.com:8443")),
            ("[2001:db8::1]:8443", Some("[2001:db8::1]:8443")),
            ("203.0.113.9", Some("203.0.113.9")),
            ("", None),
            ("a b", None),
            ("evil.com/path", None),
            ("user@evil.com", None),
            ("a\tb", None),
            ("a\"b", None),
            ("a?b", None),
            ("a#b", None),
            ("a\\b", None),
            ("example.com:8080", Some("example.com:8080")),
            ("[::1]:8080", Some("[::1]:8080")),
            ("[::1]", Some("[::1]")),
            (":80", None),
            (":", None),
            ("a:", None),
            ("[::1]:", None),
            ("[::1]x", None),
            ("[::1]:x", None),
            ("[1.2.3.4]", None),
            ("[1.2.3.4]:80", None),
            ("[::1", None),
            ("example.com:99999", None),
            ("example.com:+80", None),
            ("example.com:80:80", None),
            ("::1", None),
        ] {
            let got = resolve(
                &cfg,
                &[
                    ("x-forwarded-for", "203.0.113.9"),
                    ("x-forwarded-host", value),
                ],
            );
            assert_eq!(got.host.as_deref(), want, "{value:?}");
        }
        let got = resolve(&cfg, &[("forwarded", "for=203.0.113.9;host=\"a/b\"")]);
        assert_eq!(got.host, None);
        let got = resolve(&cfg, &[("forwarded", "for=203.0.113.9;host=example.com")]);
        assert_eq!(got.host.as_deref(), Some("example.com"));
    }

    #[test]
    fn x_forwarded_proto_and_host_lists_follow_the_client_hop() {
        let cfg = framework(&["10.0.0.0/8"]);
        // Same length as X-Forwarded-For: the value at the client's position.
        let got = resolve(
            &cfg,
            &[
                ("x-forwarded-for", "203.0.113.9, 10.0.0.7"),
                ("x-forwarded-proto", "https, http"),
                ("x-forwarded-host", "example.com, inner"),
            ],
        );
        assert_eq!(got.scheme, Scheme::Https);
        assert_eq!(got.host.as_deref(), Some("example.com"));
        // Different length: the rightmost value, set by the proxy nearest the server.
        let got = resolve(
            &cfg,
            &[
                ("x-forwarded-for", "203.0.113.9"),
                ("x-forwarded-proto", "https, http"),
                ("x-forwarded-host", "spoof.example, example.com"),
            ],
        );
        assert_eq!(got.scheme, Scheme::Http);
        assert_eq!(got.host.as_deref(), Some("example.com"));
    }
}
