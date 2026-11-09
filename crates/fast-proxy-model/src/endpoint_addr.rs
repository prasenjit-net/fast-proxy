use std::fmt;
use std::net::{IpAddr, SocketAddr};

use crate::host_pattern::validate_dns_name;
use crate::FieldError;

/// An upstream endpoint address: a hostname or IP plus a port 1-65535.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EndpointAddr {
    host: String,
    port: u16,
}

impl EndpointAddr {
    /// Parses `host:port` (IPv6 as `[::1]:port`). Hostnames are lowercased.
    pub fn parse(raw: &str) -> Result<Self, FieldError> {
        let invalid = |message: &str| Err(FieldError::new("addr", message));
        if let Ok(socket) = raw.parse::<SocketAddr>() {
            return Self::from_parts(socket.ip().to_string(), socket.port());
        }
        let Some((host, port)) = raw.rsplit_once(':') else {
            return invalid("must be host:port");
        };
        if host.parse::<IpAddr>().is_ok() || host.starts_with('[') {
            return invalid("IPv6 addresses must be written as [address]:port");
        }
        let Ok(port) = port.parse::<u16>() else {
            return invalid("port must be a number from 1 to 65535");
        };
        let host = host.to_ascii_lowercase();
        // A bare IPv4 address is valid here (unlike in host patterns), but
        // it was already handled by the SocketAddr parse above.
        validate_dns_name(&host).map_err(|err| FieldError::new("addr", err.message))?;
        Self::from_parts(host, port)
    }

    fn from_parts(host: String, port: u16) -> Result<Self, FieldError> {
        if port == 0 {
            return Err(FieldError::new("addr", "port must be from 1 to 65535"));
        }
        Ok(Self { host, port })
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn port(&self) -> u16 {
        self.port
    }
}

impl fmt::Display for EndpointAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.host.contains(':') {
            write!(f, "[{}]:{}", self.host, self.port)
        } else {
            write!(f, "{}:{}", self.host, self.port)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_hostnames_and_ip_literals() {
        for (raw, shown) in [
            ("backend:8080", "backend:8080"),
            ("Backend.Internal:80", "backend.internal:80"),
            ("127.0.0.1:3000", "127.0.0.1:3000"),
            ("[::1]:3000", "[::1]:3000"),
            ("localhost:65535", "localhost:65535"),
        ] {
            assert_eq!(
                EndpointAddr::parse(raw).unwrap().to_string(),
                shown,
                "{raw}"
            );
        }
    }

    #[test]
    fn exposes_host_and_port() {
        let addr = EndpointAddr::parse("[::1]:3000").unwrap();
        assert_eq!(addr.host(), "::1");
        assert_eq!(addr.port(), 3000);
    }

    #[test]
    fn rejects_invalid_addresses_with_an_addr_field_error() {
        for raw in [
            "",
            "backend",
            "backend:",
            ":80",
            "backend:0",
            "backend:65536",
            "backend:http",
            "::1:80",
            "[::1]",
            "bad_host:80",
            "-x:80",
            "host name:80",
            "http://backend:80",
        ] {
            let err = EndpointAddr::parse(raw).unwrap_err();
            assert_eq!(err.field, "addr", "{raw:?}");
        }
    }
}
