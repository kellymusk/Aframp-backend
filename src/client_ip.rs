//! Resolving the client's address for rate limiting and audit logs.

use std::net::{IpAddr, SocketAddr};

use axum::extract::ConnectInfo;
use axum::http::{Extensions, HeaderMap};

/// The client's IP. By default this is the socket peer: anything in a
/// request header could have been typed by the client. Behind a reverse
/// proxy (`TRUST_PROXY_HEADERS=true`) the peer is the proxy, so the address
/// comes from `X-Forwarded-For` instead, and only from its *rightmost* entry,
/// the one our proxy appended. Earlier entries are whatever the client sent.
pub fn client_ip(headers: &HeaderMap, extensions: &Extensions, trust_proxy_headers: bool) -> Option<IpAddr> {
    let peer = extensions.get::<ConnectInfo<SocketAddr>>().map(|ConnectInfo(addr)| addr.ip());
    if !trust_proxy_headers {
        return peer;
    }
    headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .filter_map(|entry| entry.trim().parse::<IpAddr>().ok())
        .last()
        .or(peer)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_peer(ip: &str) -> Extensions {
        let mut extensions = Extensions::new();
        extensions.insert(ConnectInfo(SocketAddr::new(ip.parse().unwrap(), 4000)));
        extensions
    }

    fn forwarded(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", value.parse().unwrap());
        headers
    }

    #[test]
    fn untrusted_headers_are_ignored() {
        let ip = client_ip(&forwarded("203.0.113.9"), &with_peer("10.0.0.2"), false);
        assert_eq!(ip, Some("10.0.0.2".parse().unwrap()));
    }

    #[test]
    fn trusted_proxy_uses_the_rightmost_entry_not_the_client_supplied_one() {
        // The client sent "1.2.3.4"; our proxy appended the real peer.
        let ip = client_ip(&forwarded("1.2.3.4, 198.51.100.7"), &with_peer("10.0.0.2"), true);
        assert_eq!(ip, Some("198.51.100.7".parse().unwrap()));
    }

    #[test]
    fn trusted_proxy_without_a_usable_header_falls_back_to_the_peer() {
        let ip = client_ip(&forwarded("not-an-ip"), &with_peer("10.0.0.2"), true);
        assert_eq!(ip, Some("10.0.0.2".parse().unwrap()));
        assert_eq!(client_ip(&HeaderMap::new(), &Extensions::new(), true), None);
    }
}
