//! Before the CA is asked to validate, fetch each challenge the way the CA
//! will (K8). A failure here costs no CA attempt and names what went wrong:
//! the name, and the status that came back instead of the key authorization.

use std::time::Duration;

use fast_proxy_acme::{AcmeError, HttpChallenge};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::SelfCheck;

const ATTEMPTS: u32 = 3;
const PAUSE: Duration = Duration::from_millis(500);
const FETCH_TIMEOUT: Duration = Duration::from_secs(5);

/// Checks every challenge, a few times each (behind a load balancer each try
/// may reach another instance, which is the point).
pub(crate) async fn verify(mode: SelfCheck, challenges: &[HttpChallenge]) -> Result<(), AcmeError> {
    if mode == SelfCheck::Off {
        return Ok(());
    }
    for challenge in challenges {
        let mut last = String::new();
        let mut passed = false;
        for attempt in 0..ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(PAUSE).await;
            }
            match fetch(mode, challenge).await {
                Ok(()) => {
                    passed = true;
                    break;
                }
                Err(seen) => last = seen,
            }
        }
        if !passed {
            return Err(AcmeError::transient(format!(
                "self-check: http://{}/.well-known/acme-challenge/ {last}",
                challenge.name
            )));
        }
    }
    Ok(())
}

/// One fetch; `Err` says what was seen instead of the key authorization.
async fn fetch(mode: SelfCheck, challenge: &HttpChallenge) -> Result<(), String> {
    let exchange = async {
        let mut stream = match mode {
            SelfCheck::At(addr) => TcpStream::connect(addr).await,
            _ => TcpStream::connect((challenge.name.as_str(), 80)).await,
        }
        .map_err(|err| format!("could not connect: {err}"))?;
        let _ = stream.set_nodelay(true);
        let request = format!(
            "GET /.well-known/acme-challenge/{} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
            challenge.token, challenge.name
        );
        stream
            .write_all(request.as_bytes())
            .await
            .map_err(|err| format!("could not send: {err}"))?;
        let mut answer = Vec::new();
        stream
            .read_to_end(&mut answer)
            .await
            .map_err(|err| format!("could not read: {err}"))?;
        let text = String::from_utf8_lossy(&answer);
        let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
        let status = head.split_whitespace().nth(1).unwrap_or("?").to_string();
        if status == "200" && body.trim() == challenge.key_authorization {
            return Ok(());
        }
        Err(format!("answered {status} without the key authorization"))
    };
    tokio::time::timeout(FETCH_TIMEOUT, exchange)
        .await
        .unwrap_or_else(|_| Err("timed out".into()))
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;

    use fast_proxy_acme::FailureKind;
    use tokio::net::TcpListener;

    use super::*;

    fn challenge() -> HttpChallenge {
        HttpChallenge {
            name: "shop.example.com".into(),
            token: "tok".into(),
            key_authorization: "tok.thumb".into(),
        }
    }

    /// A server answering every request with `reply`, recording the request heads.
    async fn server(
        reply: &'static str,
    ) -> (SocketAddr, tokio::sync::mpsc::UnboundedReceiver<String>) {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let mut buffer = vec![0; 2048];
                let read = stream.read(&mut buffer).await.unwrap_or(0);
                let _ = sender.send(String::from_utf8_lossy(&buffer[..read]).to_string());
                let _ = stream.write_all(reply.as_bytes()).await;
            }
        });
        (addr, receiver)
    }

    #[tokio::test]
    async fn a_served_key_authorization_passes_with_the_name_in_host() {
        let (addr, mut seen) =
            server("HTTP/1.1 200 OK\r\nContent-Length: 9\r\nConnection: close\r\n\r\ntok.thumb")
                .await;
        verify(SelfCheck::At(addr), &[challenge()]).await.unwrap();
        let head = seen.recv().await.unwrap();
        assert!(
            head.starts_with("GET /.well-known/acme-challenge/tok HTTP/1.1"),
            "{head}"
        );
        assert!(head.contains("Host: shop.example.com"), "{head}");
    }

    #[tokio::test]
    async fn a_wrong_answer_fails_transiently_naming_the_status() {
        let (addr, _) = server(
            "HTTP/1.1 308 Permanent Redirect\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        )
        .await;
        let err = verify(SelfCheck::At(addr), &[challenge()])
            .await
            .unwrap_err();
        assert_eq!(err.kind, FailureKind::Transient);
        assert!(err.detail.contains("shop.example.com"), "{err}");
        assert!(err.detail.contains("answered 308"), "{err}");
    }

    #[tokio::test]
    async fn off_checks_nothing_and_a_closed_port_fails() {
        verify(SelfCheck::Off, &[challenge()]).await.unwrap();
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let closed = listener.local_addr().unwrap();
        drop(listener);
        let err = verify(SelfCheck::At(closed), &[challenge()])
            .await
            .unwrap_err();
        assert!(err.detail.contains("could not connect"), "{err}");
    }
}
