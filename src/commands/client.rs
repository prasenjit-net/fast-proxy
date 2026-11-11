//! A small client for the admin API over the HTTP stack already in the tree
//! (hyper and hyper-rustls), with a token and optional trust for the admin
//! certificate: the OS store, a CA file, or none at all.

use std::path::Path;
use std::time::Duration;

use bytes::Bytes;
use fast_proxy_tls::{CaBundle, ClientConfigs, UpstreamTrust};
use http_body_util::{BodyExt, Full};
use hyper::{Method, Request, StatusCode};
use hyper_rustls::HttpsConnectorBuilder;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioExecutor;
use serde_json::Value;

use crate::commands::failure::Failure;

/// How long a request may take before the proxy counts as unreachable.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

type Http = Client<hyper_rustls::HttpsConnector<HttpConnector>, Full<Bytes>>;

/// How the client decides whether to trust the admin certificate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trust {
    /// The operating system's roots.
    System,
    /// Only this CA (PEM text).
    CaFile(String),
    /// Anyone: for a self-signed admin certificate you chose to accept.
    Insecure,
}

impl Trust {
    /// `--ca-file` and `--insecure` as the flags give them.
    pub fn from_flags(ca_file: Option<&Path>, insecure: bool) -> Result<Self, Failure> {
        if insecure {
            return Ok(Self::Insecure);
        }
        let Some(path) = ca_file else {
            return Ok(Self::System);
        };
        std::fs::read_to_string(path)
            .map(Self::CaFile)
            .map_err(|err| {
                Failure::Usage(format!("cannot read the CA file {}: {err}", path.display()))
            })
    }
}

/// A connection to one admin plane.
pub struct AdminClient {
    base: String,
    token: Option<String>,
    http: Http,
}

impl AdminClient {
    /// `base` is the admin plane's URL, such as `https://127.0.0.1:9000`.
    pub fn new(base: &str, token: Option<String>, trust: &Trust) -> Result<Self, Failure> {
        let trust = match trust {
            Trust::System => UpstreamTrust {
                verify: true,
                ca: None,
            },
            Trust::Insecure => UpstreamTrust {
                verify: false,
                ca: None,
            },
            Trust::CaFile(pem) => UpstreamTrust {
                verify: true,
                ca: Some(CaBundle {
                    name: "admin-ca".into(),
                    pem: pem.clone(),
                    updated_at_ms: 0,
                }),
            },
        };
        let tls = ClientConfigs::new()
            .get(&trust)
            .map_err(|err| Failure::Usage(format!("cannot use that trust setting: {err}")))?;
        let connector = HttpsConnectorBuilder::new()
            .with_tls_config((*tls).clone())
            .https_or_http()
            .enable_http1()
            .build();
        Ok(Self {
            base: base.trim_end_matches('/').to_string(),
            token,
            http: Client::builder(TokioExecutor::new()).build(connector),
        })
    }

    pub async fn get(&self, path: &str) -> Result<Value, Failure> {
        self.call(Method::GET, path, None).await
    }

    pub async fn post(&self, path: &str) -> Result<Value, Failure> {
        self.call(Method::POST, path, None).await
    }

    pub async fn put(&self, path: &str, body: &Value) -> Result<Value, Failure> {
        self.call(Method::PUT, path, Some(body)).await
    }

    pub async fn delete(&self, path: &str) -> Result<Value, Failure> {
        self.call(Method::DELETE, path, None).await
    }

    /// One request. The proxy's answer comes back as JSON (`Null` when empty);
    /// a status of 400 or more becomes [`Failure::Refused`] carrying the proxy's
    /// own message, and a connection that cannot be made, [`Failure::Unreachable`].
    pub async fn call(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, Failure> {
        let url = format!("{}{path}", self.base);
        let mut builder = Request::builder().method(method).uri(&url);
        if let Some(token) = &self.token {
            builder = builder.header("authorization", format!("Bearer {token}"));
        }
        let payload = body.map(Value::to_string).unwrap_or_default();
        if body.is_some() {
            builder = builder.header("content-type", "application/json");
        }
        let request = builder
            .body(Full::new(Bytes::from(payload)))
            .map_err(|err| Failure::Usage(format!("{url} is not a usable URL: {err}")))?;
        let unreachable = |why: String| {
            Failure::Unreachable(format!("cannot reach the proxy at {}: {why}", self.base))
        };
        let response = tokio::time::timeout(REQUEST_TIMEOUT, async {
            let response = self
                .http
                .request(request)
                .await
                .map_err(|e| e.to_string())?;
            let status = response.status();
            let bytes = response
                .into_body()
                .collect()
                .await
                .map_err(|e| e.to_string())?
                .to_bytes();
            Ok::<_, String>((status, bytes))
        })
        .await
        .map_err(|_| unreachable("it did not answer in time".into()))?
        .map_err(unreachable)?;
        let (status, bytes) = response;
        let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        if status.as_u16() >= 400 {
            return Err(Failure::Refused(refusal_message(
                status,
                &json,
                self.token.is_some(),
            )));
        }
        Ok(json)
    }
}

/// What to tell the operator about a refusal, naming the proxy's reason.
fn refusal_message(status: StatusCode, body: &Value, had_token: bool) -> String {
    let reason = body["error"]["message"]
        .as_str()
        .unwrap_or("no reason given");
    let mut text = format!(
        "the proxy refused the request ({}): {reason}",
        status.as_u16()
    );
    if let Some(fields) = body["error"]["fields"].as_array() {
        for field in fields {
            text.push_str(&format!(
                "\n  {}: {}",
                field["field"].as_str().unwrap_or("?"),
                field["message"].as_str().unwrap_or("")
            ));
        }
    }
    if status == StatusCode::UNAUTHORIZED && !had_token {
        text.push_str(
            "\n  pass a token with --token-file, or set FAST_PROXY_TOKEN (the first start printed one)",
        );
    }
    text
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_refusal_names_the_proxys_reason_and_each_bad_field() {
        let body = json!({ "error": { "message": "validation failed", "fields": [
            { "field": "ttlSecs", "message": "must be between 1 and 2592000" } ] } });
        let text = refusal_message(StatusCode::BAD_REQUEST, &body, true);
        assert!(text.contains("(400): validation failed"), "{text}");
        assert!(text.contains("ttlSecs: must be between"), "{text}");
    }

    #[test]
    fn a_401_without_a_token_says_how_to_pass_one() {
        let body = json!({ "error": { "message": "a valid admin token is required" } });
        assert!(
            refusal_message(StatusCode::UNAUTHORIZED, &body, false).contains("FAST_PROXY_TOKEN")
        );
        assert!(
            !refusal_message(StatusCode::UNAUTHORIZED, &body, true).contains("FAST_PROXY_TOKEN")
        );
    }

    #[test]
    fn flags_choose_the_trust() {
        assert_eq!(Trust::from_flags(None, false).unwrap(), Trust::System);
        assert_eq!(Trust::from_flags(None, true).unwrap(), Trust::Insecure);
        let dir = tempfile::tempdir().unwrap();
        let ca = dir.path().join("ca.pem");
        std::fs::write(&ca, "PEM").unwrap();
        assert_eq!(
            Trust::from_flags(Some(&ca), false).unwrap(),
            Trust::CaFile("PEM".into())
        );
        let missing = Trust::from_flags(Some(&dir.path().join("none.pem")), false).unwrap_err();
        assert!(matches!(missing, Failure::Usage(_)));
    }

    #[tokio::test]
    async fn an_unreachable_proxy_is_reported_as_such() {
        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let client =
            AdminClient::new(&format!("http://127.0.0.1:{port}"), None, &Trust::System).unwrap();
        let err = client.get("/api/health").await.unwrap_err();
        assert!(
            matches!(&err, Failure::Unreachable(m) if m.contains("cannot reach")),
            "{err:?}"
        );
    }
}
