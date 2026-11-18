//! The `[limits]` section reaches the real binary's data plane: the per-client
//! connection cap and the request target cap (ADR 0029). The full exit-criteria
//! tests are M7 Q1.

mod common;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use common::{create_site, metric, start_backend, start_on_free_ports, through_proxy, wait_until};

const LIMITS: &str = "\n[limits]\nmax_connections_per_client = 2\nmax_uri_bytes = 256\n";

#[test]
fn the_limits_section_reaches_the_data_plane() {
    let dir = tempfile::tempdir().unwrap();
    let backend = start_backend("site");
    let proxy = start_on_free_ports(dir.path(), "", LIMITS);
    create_site(&proxy, "shop", "shop.example.com", backend);

    let long = format!("/{}", "a".repeat(256));
    assert_eq!(through_proxy(&proxy, "shop.example.com", &long).0, 414);
    assert_eq!(through_proxy(&proxy, "shop.example.com", "/").0, 200);

    let held: Vec<TcpStream> = (0..2)
        .map(|_| TcpStream::connect(proxy.data).unwrap())
        .collect();
    let mut third = TcpStream::connect(proxy.data).unwrap();
    third
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let _ = third.write_all(b"GET / HTTP/1.1\r\nHost: shop.example.com\r\n\r\n");
    let mut buf = [0u8; 64];
    let closed = matches!(third.read(&mut buf), Ok(0) | Err(_));
    assert!(closed, "the third connection from one client is closed");
    assert!(wait_until(|| {
        metric(
            &proxy,
            r#"fastproxy_connections_refused_total{reason="max_connections_per_client"}"#,
        )
        .is_some_and(|count| count >= 1.0)
    }));
    drop(held);
}
