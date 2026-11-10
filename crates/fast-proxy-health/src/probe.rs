use std::time::Duration;

use std::sync::Arc;

use fast_proxy_model::{EndpointAddr, HealthCheck, HealthKind};
use rustls::pki_types::ServerName;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

/// How many bytes of the response to read while looking for the status line.
const STATUS_LINE_LIMIT: usize = 512;

/// How an HTTP probe talks TLS to an endpoint of a TLS group: the group's own
/// client configuration (so it tests the path real traffic takes) and the name
/// to verify. The TCP probe stays a plain connect.
#[derive(Clone)]
pub struct ProbeTls {
    pub config: Arc<rustls::ClientConfig>,
    pub server_name: ServerName<'static>,
}

impl std::fmt::Debug for ProbeTls {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProbeTls")
            .field("server_name", &self.server_name)
            .finish_non_exhaustive()
    }
}

/// Equal when both use the same configuration object and name: a replaced CA
/// bundle is a new configuration, so the prober restarts with it.
impl PartialEq for ProbeTls {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.config, &other.config) && self.server_name == other.server_name
    }
}

impl Eq for ProbeTls {}

/// What to run against an endpoint, taken from the group's health settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeSpec {
    pub kind: HealthKind,
    pub path: String,
    pub status_min: u16,
    pub status_max: u16,
    pub timeout: Duration,
}

impl ProbeSpec {
    /// The probe part of a group's health settings.
    pub fn from_check(check: &HealthCheck) -> Self {
        Self {
            kind: check.kind,
            path: check.path.clone(),
            status_min: check.status_min,
            status_max: check.status_max,
            timeout: check.timeout,
        }
    }
}

/// Probes `addr` once: `Ok` if it is healthy, otherwise why not. A TCP probe
/// only needs the connection; an HTTP probe sends `GET <path>` and wants a
/// status inside the expected range. The whole probe is bounded by the timeout.
pub async fn run_probe(
    spec: &ProbeSpec,
    addr: &EndpointAddr,
    tls: Option<&ProbeTls>,
) -> Result<(), String> {
    let probing = async {
        let stream = TcpStream::connect(addr.to_string())
            .await
            .map_err(|err| format!("connect failed: {err}"))?;
        match (spec.kind, tls) {
            (HealthKind::Http, Some(tls)) => {
                let mut secure = TlsConnector::from(tls.config.clone())
                    .connect(tls.server_name.clone(), stream)
                    .await
                    .map_err(|err| format!("TLS handshake failed: {err}"))?;
                http_status_check(&mut secure, spec, addr).await
            }
            (HealthKind::Http, None) => {
                let mut stream = stream;
                http_status_check(&mut stream, spec, addr).await
            }
            (HealthKind::Tcp | HealthKind::None, _) => Ok(()),
        }
    };
    tokio::time::timeout(spec.timeout, probing)
        .await
        .unwrap_or_else(|_| Err(format!("no answer within {} ms", spec.timeout.as_millis())))
}

async fn http_status_check<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    spec: &ProbeSpec,
    addr: &EndpointAddr,
) -> Result<(), String> {
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {addr}\r\nUser-Agent: fast-proxy-health\r\nConnection: close\r\n\r\n",
        spec.path
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|err| format!("send failed: {err}"))?;
    let mut head = Vec::new();
    let mut chunk = [0_u8; 128];
    while !head.contains(&b'\n') && head.len() < STATUS_LINE_LIMIT {
        let read = stream
            .read(&mut chunk)
            .await
            .map_err(|err| format!("read failed: {err}"))?;
        if read == 0 {
            break;
        }
        head.extend_from_slice(&chunk[..read]);
    }
    let status = parse_status(&head).ok_or_else(|| "no HTTP status line".to_string())?;
    if (spec.status_min..=spec.status_max).contains(&status) {
        Ok(())
    } else {
        Err(format!(
            "status {status} outside {}-{}",
            spec.status_min, spec.status_max
        ))
    }
}

/// The status code in an `HTTP/1.x NNN ...` line.
fn parse_status(head: &[u8]) -> Option<u16> {
    let line = head.split(|byte| *byte == b'\n').next()?;
    let text = std::str::from_utf8(line).ok()?;
    let mut parts = text.split_whitespace();
    parts
        .next()
        .filter(|version| version.starts_with("HTTP/1."))?;
    parts.next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use tokio::net::TcpListener;

    use super::*;

    fn spec(kind: HealthKind) -> ProbeSpec {
        ProbeSpec {
            kind,
            path: "/healthz".into(),
            status_min: 200,
            status_max: 399,
            timeout: Duration::from_millis(300),
        }
    }

    /// A server answering every connection with `reply` (raw bytes), which it
    /// reports the request line of.
    async fn server(
        reply: &'static str,
    ) -> (EndpointAddr, tokio::sync::mpsc::UnboundedReceiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = EndpointAddr::parse(&listener.local_addr().unwrap().to_string()).unwrap();
        let (seen, requests) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    return;
                };
                let seen = seen.clone();
                tokio::spawn(async move {
                    let mut buffer = [0_u8; 1024];
                    let read = stream.read(&mut buffer).await.unwrap_or(0);
                    let text = String::from_utf8_lossy(&buffer[..read]).to_string();
                    let _ = seen.send(text.lines().next().unwrap_or("").to_string());
                    let _ = stream.write_all(reply.as_bytes()).await;
                });
            }
        });
        (addr, requests)
    }

    #[tokio::test]
    async fn an_http_probe_sends_a_get_for_the_path_and_accepts_a_status_in_range() {
        let (addr, mut requests) = server("HTTP/1.1 204 No Content\r\n\r\n").await;
        assert_eq!(
            run_probe(&spec(HealthKind::Http), &addr, None).await,
            Ok(())
        );
        assert_eq!(requests.recv().await.unwrap(), "GET /healthz HTTP/1.1");
    }

    #[tokio::test]
    async fn an_http_probe_fails_outside_the_expected_range_and_says_why() {
        for (reply, status) in [
            ("HTTP/1.1 500 Oops\r\n\r\n", 500),
            ("HTTP/1.1 404 No\r\n\r\n", 404),
        ] {
            let (addr, _) = server(reply).await;
            let err = run_probe(&spec(HealthKind::Http), &addr, None)
                .await
                .unwrap_err();
            assert!(err.contains(&status.to_string()), "{err}");
        }
        let (addr, _) = server("HTTP/1.1 301 Moved\r\n\r\n").await;
        let narrow = ProbeSpec {
            status_max: 299,
            ..spec(HealthKind::Http)
        };
        assert!(
            run_probe(&narrow, &addr, None).await.is_err(),
            "301 is outside 200-299"
        );
    }

    #[tokio::test]
    async fn garbage_instead_of_a_status_line_fails() {
        let (addr, _) = server("not http at all\r\n").await;
        let err = run_probe(&spec(HealthKind::Http), &addr, None)
            .await
            .unwrap_err();
        assert!(err.contains("status line"), "{err}");
    }

    #[tokio::test]
    async fn a_tcp_probe_only_needs_the_connection() {
        let (addr, _) = server("whatever").await;
        assert_eq!(run_probe(&spec(HealthKind::Tcp), &addr, None).await, Ok(()));
    }

    #[tokio::test]
    async fn a_refused_connection_fails_either_probe() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = EndpointAddr::parse(&listener.local_addr().unwrap().to_string()).unwrap();
        drop(listener);
        // Windows takes about a second to report a refused connection, so
        // allow far longer than the other tests' timeout; Linux and macOS
        // answer at once.
        let patient = |kind| ProbeSpec {
            timeout: Duration::from_secs(10),
            ..spec(kind)
        };
        for kind in [HealthKind::Tcp, HealthKind::Http] {
            let err = run_probe(&patient(kind), &addr, None).await.unwrap_err();
            assert!(err.contains("connect failed"), "{err}");
        }
    }

    #[tokio::test]
    async fn a_server_that_never_answers_times_out() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = EndpointAddr::parse(&listener.local_addr().unwrap().to_string()).unwrap();
        tokio::spawn(async move {
            let _held = listener.accept().await;
            tokio::time::sleep(Duration::from_secs(5)).await;
        });
        let err = run_probe(&spec(HealthKind::Http), &addr, None)
            .await
            .unwrap_err();
        assert!(err.contains("no answer within 300 ms"), "{err}");
    }

    #[test]
    fn status_lines_are_parsed_strictly() {
        assert_eq!(parse_status(b"HTTP/1.1 200 OK\r\nx: y"), Some(200));
        assert_eq!(parse_status(b"HTTP/1.0 503"), Some(503));
        assert_eq!(parse_status(b"HTTP/2 200"), None);
        assert_eq!(parse_status(b"GET / HTTP/1.1"), None);
        assert_eq!(parse_status(b""), None);
        assert_eq!(parse_status(b"HTTP/1.1 abc"), None);
    }

    #[test]
    fn the_spec_comes_from_the_groups_health_settings() {
        let check = HealthCheck {
            kind: HealthKind::Http,
            path: "/up".into(),
            status_min: 200,
            status_max: 200,
            ..HealthCheck::default()
        };
        let spec = ProbeSpec::from_check(&check);
        assert_eq!(
            (spec.kind, spec.path.as_str(), spec.status_max),
            (HealthKind::Http, "/up", 200)
        );
        assert_eq!(spec.timeout, check.timeout);
    }

    /// A TLS server answering every HTTP request with `reply`, its certificate
    /// issued by `ca` for `names`.
    async fn tls_server(
        ca: &fast_proxy_test_pki::TestCa,
        names: &[&str],
        reply: &'static str,
    ) -> EndpointAddr {
        use rustls::pki_types::pem::PemObject;
        let issued = ca.issue(&fast_proxy_test_pki::LeafSpec::new(names));
        let chain: Vec<_> =
            rustls::pki_types::CertificateDer::pem_slice_iter(issued.chain_pem.as_bytes())
                .collect::<Result<_, _>>()
                .unwrap();
        let key =
            rustls::pki_types::PrivateKeyDer::from_pem_slice(issued.key_pem.as_bytes()).unwrap();
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .unwrap();
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = EndpointAddr::parse(&listener.local_addr().unwrap().to_string()).unwrap();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let acceptor = acceptor.clone();
                tokio::spawn(async move {
                    if let Ok(mut tls) = acceptor.accept(stream).await {
                        let mut buffer = [0_u8; 1024];
                        let _ = tls.read(&mut buffer).await;
                        let _ = tls.write_all(reply.as_bytes()).await;
                        let _ = tls.shutdown().await;
                    }
                });
            }
        });
        addr
    }

    fn probe_tls(ca: &fast_proxy_test_pki::TestCa, name: &str) -> ProbeTls {
        let configs = fast_proxy_tls::ClientConfigs::new();
        let config = configs
            .get(&fast_proxy_tls::UpstreamTrust {
                verify: true,
                ca: Some(fast_proxy_tls::CaBundle {
                    name: "corp".into(),
                    pem: ca.pem.clone(),
                    updated_at_ms: 1,
                }),
            })
            .unwrap();
        ProbeTls {
            config,
            server_name: fast_proxy_tls::server_name_for("", name).unwrap(),
        }
    }

    fn patient_http() -> ProbeSpec {
        ProbeSpec {
            timeout: Duration::from_secs(5),
            ..spec(HealthKind::Http)
        }
    }

    #[tokio::test]
    async fn an_http_probe_over_tls_marks_a_trusted_endpoint_healthy() {
        let ca = fast_proxy_test_pki::TestCa::new("corp root");
        let addr = tls_server(
            &ca,
            &["backend.internal"],
            "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n",
        )
        .await;
        let tls = probe_tls(&ca, "backend.internal");
        assert_eq!(run_probe(&patient_http(), &addr, Some(&tls)).await, Ok(()));
    }

    #[tokio::test]
    async fn an_untrusted_certificate_a_wrong_name_and_a_plain_port_are_unhealthy() {
        let ca = fast_proxy_test_pki::TestCa::new("corp root");
        let addr = tls_server(&ca, &["backend.internal"], "HTTP/1.1 200 OK\r\n\r\n").await;

        let other = fast_proxy_test_pki::TestCa::new("someone else");
        let err = run_probe(
            &patient_http(),
            &addr,
            Some(&probe_tls(&other, "backend.internal")),
        )
        .await
        .unwrap_err();
        assert!(err.contains("TLS handshake failed"), "{err}");

        let err = run_probe(
            &patient_http(),
            &addr,
            Some(&probe_tls(&ca, "wrong.internal")),
        )
        .await
        .unwrap_err();
        assert!(err.contains("TLS handshake failed"), "{err}");

        let (plain, _requests) = server("HTTP/1.1 200 OK\r\n\r\n").await;
        let err = run_probe(
            &patient_http(),
            &plain,
            Some(&probe_tls(&ca, "backend.internal")),
        )
        .await
        .unwrap_err();
        assert!(
            err.contains("TLS handshake failed"),
            "a plain-HTTP port is not a TLS endpoint: {err}"
        );
    }

    #[tokio::test]
    async fn a_tcp_probe_of_a_tls_group_stays_a_plain_connect() {
        let ca = fast_proxy_test_pki::TestCa::new("corp root");
        let addr = tls_server(&ca, &["backend.internal"], "").await;
        let other = fast_proxy_test_pki::TestCa::new("someone else");
        let tls = probe_tls(&other, "backend.internal");
        assert_eq!(
            run_probe(&spec(HealthKind::Tcp), &addr, Some(&tls)).await,
            Ok(())
        );
    }

    #[test]
    fn tls_settings_compare_by_configuration_and_name() {
        let ca = fast_proxy_test_pki::TestCa::new("corp root");
        let one = probe_tls(&ca, "backend.internal");
        assert_eq!(one, one.clone());
        let other_name = ProbeTls {
            server_name: fast_proxy_tls::server_name_for("", "other.internal").unwrap(),
            ..one.clone()
        };
        assert_ne!(one, other_name);
        let other_config = probe_tls(&ca, "backend.internal");
        assert_eq!(other_config.server_name, one.server_name);
        assert_ne!(
            one, other_config,
            "a separately built configuration is a different one"
        );
    }
}
