// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: The Paper Plane authors

//! Parsing of `t.me/proxy` / `tg://proxy` links into proxy connection
//! parameters, mirroring the links Telegram shares for MTProto proxies.
//!

use std::borrow::Cow;

/// The proxy protocol a [`ProxyLink`] describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProxyKindTag {
    Mtproto,
    Socks5,
    Http,
}

/// Connection parameters decoded from a proxy link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProxyLink {
    pub(crate) server: String,
    pub(crate) port: u16,
    pub(crate) kind: ProxyKindTag,
    pub(crate) secret: String,
    pub(crate) username: String,
    pub(crate) password: String,
    /// Display name taken from the link's `#fragment`, if any.
    pub(crate) name: Option<String>,
}

/// Recognised hosts carrying the `/proxy` path.
const PROXY_HOSTS: &[&str] = &["t.me", "telegram.me", "www.t.me", "proxy.t.me"];

/// Parses a proxy link URI. Returns `None` for anything that is not a
/// well-formed proxy link, so callers can fall through to normal URL
/// handling.
pub(crate) fn parse_proxy_link(uri: &str) -> Option<ProxyLink> {
    let (rest, name) = match uri.split_once('#') {
        Some((rest, fragment)) => (rest, Some(percent_decode(fragment).into_owned())),
        None => (uri, None),
    };

    let query;
    if let Some(after_scheme) = rest.strip_prefix("tg://") {
        // tg://proxy?server=…
        let (host, q) = after_scheme.split_once('?')?;
        if !host.eq_ignore_ascii_case("proxy") && !host.eq_ignore_ascii_case("proxy/") {
            return None;
        }
        query = q;
    } else {
        let without_scheme = rest
            .strip_prefix("https://")
            .or_else(|| rest.strip_prefix("http://"))?;
        let (host, path_and_query) = without_scheme
            .split_once('/')
            .unwrap_or((without_scheme, ""));
        if !PROXY_HOSTS.iter().any(|h| h.eq_ignore_ascii_case(host)) {
            return None;
        }
        let (path, q) = path_and_query.split_once('?')?;
        if !path.eq_ignore_ascii_case("proxy") {
            return None;
        }
        query = q;
    }

    let mut server = None;
    let mut port = None;
    let mut secret = None;
    let mut username = String::new();
    let mut password = String::new();
    let mut declared_type = None;

    for pair in query.split('&') {
        let (key, value) = pair.split_once('=')?;
        let value = percent_decode(value);
        match key {
            "server" => server = Some(value.into_owned()),
            "port" => port = Some(value.parse::<u16>().ok()?),
            "secret" => secret = Some(value.into_owned()),
            "username" => username = value.into_owned(),
            "password" => password = value.into_owned(),
            "type" => declared_type = Some(value.into_owned()),
            _ => {}
        }
    }

    let server = server.filter(|s| !s.is_empty())?;
    // IPv6 hosts arrive bracketed (%5B…%5D); TDLib wants the bare address.
    let server = server
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(&server)
        .to_string();
    let port = port?;

    let kind = match declared_type.as_deref() {
        Some(t) if t.eq_ignore_ascii_case("socks5") => ProxyKindTag::Socks5,
        Some(t) if t.eq_ignore_ascii_case("http") => ProxyKindTag::Http,
        Some(t) if t.eq_ignore_ascii_case("mtproto") => ProxyKindTag::Mtproto,
        Some(_) => return None,
        // No explicit type: only an MTProto secret makes it a proxy link.
        None if secret.is_some() => ProxyKindTag::Mtproto,
        None => return None,
    };

    let secret = secret.unwrap_or_default();
    if kind == ProxyKindTag::Mtproto && !is_even_hex(&secret) {
        return None;
    }

    Some(ProxyLink {
        server,
        port,
        kind,
        secret,
        username,
        password,
        name,
    })
}

fn is_even_hex(value: &str) -> bool {
    !value.is_empty()
        && value.len().is_multiple_of(2)
        && value.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Minimal percent-decoding; avoids pulling in a dependency for one use.
fn percent_decode(value: &str) -> Cow<'_, str> {
    if !value.contains('%') {
        return Cow::Borrowed(value);
    }
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex2byte(bytes[i + 1]), hex2byte(bytes[i + 2]))
        {
            out.push(hi << 4 | lo);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    // The decoded bytes are local, so hand back an owned Cow.
    Cow::Owned(String::from_utf8_lossy(&out).into_owned())
}

fn hex2byte(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_telegram_me_mtproto() {
        let link = parse_proxy_link("https://t.me/proxy?server=1.2.3.4&port=443&secret=eeF0ABC123")
            .expect("valid mtproto link");
        assert_eq!(link.server, "1.2.3.4");
        assert_eq!(link.port, 443);
        assert_eq!(link.kind, ProxyKindTag::Mtproto);
        assert_eq!(link.secret, "eeF0ABC123");
        assert_eq!(link.name, None);
    }

    #[test]
    fn parses_tg_scheme_and_fragment_name() {
        let link = parse_proxy_link("tg://proxy?server=example.com&port=8443&secret=f0f0#MyProx")
            .expect("tg:// scheme");
        assert_eq!(link.server, "example.com");
        assert_eq!(link.port, 8443);
        assert_eq!(link.kind, ProxyKindTag::Mtproto);
        assert_eq!(link.name.as_deref(), Some("MyProx"));
    }

    #[test]
    fn parses_ipv6_brackets_and_percent_encoding() {
        let link = parse_proxy_link(
            "https://t.me/proxy?server=%5B2001%3Adb8%3A%3A1%5D&port=443&secret=ee01",
        )
        .expect("encoded ipv6 server");
        assert_eq!(link.server, "2001:db8::1");
    }

    #[test]
    fn parses_explicit_socks5_and_http_types() {
        let socks = parse_proxy_link(
            "https://t.me/proxy?server=h&port=1080&type=socks5&username=u&password=p",
        )
        .expect("socks5 type");
        assert_eq!(socks.kind, ProxyKindTag::Socks5);
        assert_eq!(socks.username, "u");
        assert_eq!(socks.password, "p");

        let http =
            parse_proxy_link("https://t.me/proxy?server=h&port=3128&type=http").expect("http type");
        assert_eq!(http.kind, ProxyKindTag::Http);
    }

    #[test]
    fn rejects_bad_input() {
        // missing port / server / secret
        assert_eq!(parse_proxy_link("https://t.me/proxy?server=1.2.3.4"), None);
        assert_eq!(
            parse_proxy_link("https://t.me/proxy?port=443&secret=ee01"),
            None
        );
        // non-numeric / out-of-range port
        assert_eq!(
            parse_proxy_link("https://t.me/proxy?server=h&port=abc&secret=ee01"),
            None
        );
        assert_eq!(
            parse_proxy_link("https://t.me/proxy?server=h&port=70000&secret=ee01"),
            None
        );
        // odd-length and non-hex secrets
        assert_eq!(
            parse_proxy_link("https://t.me/proxy?server=h&port=443&secret=abc"),
            None
        );
        assert_eq!(
            parse_proxy_link("https://t.me/proxy?server=h&port=443&secret=zz01"),
            None
        );
        // foreign host / plain url
        assert_eq!(
            parse_proxy_link("https://example.com/proxy?server=h&port=443&secret=ee01"),
            None
        );
        assert_eq!(parse_proxy_link("https://example.com/"), None);
        // no type and no secret is not a proxy link
        assert_eq!(
            parse_proxy_link("https://t.me/proxy?server=h&port=1080&username=u"),
            None
        );
    }
}
