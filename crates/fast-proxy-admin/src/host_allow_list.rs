//! The `Host` allow-list that stops DNS rebinding: a page on `evil.example`
//! whose name resolves to 127.0.0.1 can reach a loopback admin plane, but its
//! requests carry `Host: evil.example`, which is refused here.

use std::collections::HashSet;
use std::net::SocketAddr;

/// The host names the admin plane answers to.
#[derive(Debug, Clone)]
pub struct HostAllowList {
    names: HashSet<String>,
}

impl HostAllowList {
    /// The listen address (unless unspecified) and the loopback names are
    /// always allowed; `extra` adds more.
    pub fn new(listen: SocketAddr, extra: &[String]) -> Self {
        let mut names: HashSet<String> = ["localhost", "127.0.0.1", "[::1]"]
            .into_iter()
            .map(String::from)
            .collect();
        if !listen.ip().is_unspecified() {
            names.insert(host_only(&listen.ip().to_string()));
            names.insert(bracketed(&listen));
        }
        names.extend(extra.iter().map(|name| host_only(name)));
        Self { names }
    }

    /// Whether a `Host` header value (`name`, `name:port`, `[v6]:port`) is allowed.
    pub fn allows(&self, host_header: &str) -> bool {
        self.names.contains(&host_only(host_header))
    }
}

fn bracketed(listen: &SocketAddr) -> String {
    match listen {
        SocketAddr::V4(v4) => v4.ip().to_string(),
        SocketAddr::V6(v6) => format!("[{}]", v6.ip()),
    }
}

/// Lowercases and drops the port: `Example.com:9000` is `example.com`,
/// `[::1]:9000` is `[::1]`.
fn host_only(value: &str) -> String {
    let value = value.trim().to_ascii_lowercase();
    if let Some(rest) = value.strip_prefix('[') {
        return match rest.split_once(']') {
            Some((address, _)) => format!("[{address}]"),
            None => value,
        };
    }
    match value.rsplit_once(':') {
        // A bare IPv6 address has several colons and no port to drop.
        Some((name, port)) if !name.contains(':') && port.bytes().all(|b| b.is_ascii_digit()) => {
            name.to_string()
        }
        _ => value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(listen: &str, extra: &[&str]) -> HostAllowList {
        let extra: Vec<String> = extra.iter().map(ToString::to_string).collect();
        HostAllowList::new(listen.parse().unwrap(), &extra)
    }

    #[test]
    fn loopback_names_are_always_allowed_with_or_without_a_port() {
        let allowed = list("127.0.0.1:9000", &[]);
        for host in [
            "localhost",
            "localhost:9000",
            "LOCALHOST:1",
            "127.0.0.1",
            "127.0.0.1:9000",
            "[::1]",
            "[::1]:9000",
        ] {
            assert!(allowed.allows(host), "{host}");
        }
    }

    #[test]
    fn other_names_are_refused() {
        let allowed = list("127.0.0.1:9000", &[]);
        for host in [
            "evil.example",
            "evil.example:9000",
            "localhost.evil.example",
            "127.0.0.2",
            "",
            "[::2]:9000",
            "localhost:80@evil.example",
        ] {
            assert!(!allowed.allows(host), "{host}");
        }
    }

    #[test]
    fn the_listen_address_and_configured_names_are_allowed() {
        let allowed = list(
            "192.0.2.10:9000",
            &["Admin.Example.com", "ops.internal:8443"],
        );
        for host in [
            "192.0.2.10:9000",
            "admin.example.com",
            "ADMIN.example.com:9000",
            "ops.internal",
        ] {
            assert!(allowed.allows(host), "{host}");
        }
    }

    #[test]
    fn an_unspecified_listen_address_allows_no_extra_name() {
        let allowed = list("0.0.0.0:9000", &[]);
        assert!(!allowed.allows("0.0.0.0"));
        assert!(allowed.allows("localhost"));
    }

    #[test]
    fn an_ipv6_listen_address_is_allowed_bracketed() {
        let allowed = list("[2001:db8::1]:9000", &[]);
        assert!(allowed.allows("[2001:db8::1]:9000"));
        assert!(allowed.allows("[2001:db8::1]"));
    }
}
