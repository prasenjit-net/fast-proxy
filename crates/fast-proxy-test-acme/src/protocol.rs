//! The ACME resources (RFC 8555 section 7): directory, nonces, accounts,
//! orders, authorizations, HTTP-01 challenges, finalize and certificates.

use std::net::SocketAddr;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Method, StatusCode};
use axum::response::Response;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use fast_proxy_test_pki::LeafSpec;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::server::Shared;
use crate::state::{Account, Authorization, Order, State, Status};
use crate::Validation;

const PROBLEM: &str = "urn:ietf:params:acme:error:";

/// One request, decoded from its JWS envelope where it has one.
struct Signed {
    account: Option<usize>,
    jwk: Option<Value>,
    payload: Value,
}

pub(crate) async fn handle(state: &Shared, method: &Method, path: &str, body: &[u8]) -> Response {
    let mut guard = state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let state_ref = &mut *guard;
    if *method == Method::GET && path == "/dir" {
        return json_reply(state_ref, StatusCode::OK, directory(&state_ref.base), None);
    }
    if path == "/nonce" {
        return json_reply(state_ref, StatusCode::OK, Value::Null, None);
    }
    let Some(signed) = decode(state_ref, body) else {
        return problem(
            state_ref,
            StatusCode::BAD_REQUEST,
            "malformed",
            "not a JWS request",
        );
    };
    let (resource, id) = match path.trim_start_matches('/').split_once('/') {
        Some((resource, id)) => (resource, id.parse::<usize>().ok()),
        None => (path.trim_start_matches('/'), None),
    };
    match (resource, id) {
        ("account", None) => new_account(state_ref, &signed),
        ("order", None) => new_order(state_ref, &signed),
        ("order", Some(id)) if id < state_ref.orders.len() => {
            let body = order_json(state_ref, id);
            json_reply(state_ref, StatusCode::OK, body, None)
        }
        ("authz", Some(id)) if id < state_ref.authorizations.len() => {
            let body = authorization_json(state_ref, id);
            json_reply(state_ref, StatusCode::OK, body, None)
        }
        ("chall", Some(id)) if id < state_ref.authorizations.len() => {
            let reply = start_validation(state_ref, id, state.clone());
            drop(guard);
            reply
        }
        ("finalize", Some(id)) if id < state_ref.orders.len() => finalize(state_ref, id, &signed),
        ("cert", Some(id)) if id < state_ref.certificates.len() => {
            let pem = state_ref.certificates[id].clone();
            let nonce = state_ref.nonce();
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "application/pem-certificate-chain")
                .header("replay-nonce", nonce)
                .body(Body::from(pem))
                .expect("a valid response")
        }
        _ => problem(
            state_ref,
            StatusCode::NOT_FOUND,
            "malformed",
            "no such resource",
        ),
    }
}

fn directory(base: &str) -> Value {
    json!({
        "newNonce": format!("{base}/nonce"),
        "newAccount": format!("{base}/account"),
        "newOrder": format!("{base}/order"),
        "revokeCert": format!("{base}/revoke"),
        "keyChange": format!("{base}/key-change"),
    })
}

fn decode(state: &State, body: &[u8]) -> Option<Signed> {
    let envelope: Value = serde_json::from_slice(body).ok()?;
    let protected: Value =
        serde_json::from_slice(&b64(envelope.get("protected")?.as_str()?)?).ok()?;
    let payload = match envelope.get("payload")?.as_str()? {
        "" => Value::Null,
        encoded => serde_json::from_slice(&b64(encoded)?).ok()?,
    };
    let account = protected
        .get("kid")
        .and_then(Value::as_str)
        .and_then(|kid| kid.strip_prefix(&format!("{}/acct/", state.base)))
        .and_then(|id| id.parse().ok());
    Some(Signed {
        account,
        jwk: protected.get("jwk").cloned(),
        payload,
    })
}

fn b64(text: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(text).ok()
}

/// The RFC 7638 thumbprint of an EC public key.
fn thumbprint(jwk: &Value) -> Option<String> {
    let canonical = format!(
        r#"{{"crv":"{}","kty":"{}","x":"{}","y":"{}"}}"#,
        jwk.get("crv")?.as_str()?,
        jwk.get("kty")?.as_str()?,
        jwk.get("x")?.as_str()?,
        jwk.get("y")?.as_str()?,
    );
    Some(URL_SAFE_NO_PAD.encode(Sha256::digest(canonical.as_bytes())))
}

fn new_account(state: &mut State, signed: &Signed) -> Response {
    let Some(thumbprint) = signed.jwk.as_ref().and_then(thumbprint) else {
        return problem(
            state,
            StatusCode::BAD_REQUEST,
            "malformed",
            "a new account needs a jwk",
        );
    };
    state.accounts.push(Account { thumbprint });
    let location = format!("{}/acct/{}", state.base, state.accounts.len() - 1);
    let contact = signed.payload.get("contact").cloned().unwrap_or(json!([]));
    json_reply(
        state,
        StatusCode::CREATED,
        json!({ "status": "valid", "contact": contact }),
        Some(location),
    )
}

fn new_order(state: &mut State, signed: &Signed) -> Response {
    let Some(account) = signed.account.filter(|id| *id < state.accounts.len()) else {
        return problem(
            state,
            StatusCode::UNAUTHORIZED,
            "accountDoesNotExist",
            "unknown account",
        );
    };
    if state.rate_limited {
        return problem(
            state,
            StatusCode::TOO_MANY_REQUESTS,
            "rateLimited",
            "too many new orders recently",
        );
    }
    let names: Vec<String> = signed
        .payload
        .get("identifiers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|identifier| identifier.get("value")?.as_str().map(str::to_string))
        .collect();
    if names.is_empty() {
        return problem(
            state,
            StatusCode::BAD_REQUEST,
            "malformed",
            "no identifiers",
        );
    }
    let authorizations = names
        .iter()
        .map(|name| {
            let id = state.authorizations.len();
            state.authorizations.push(Authorization {
                account,
                name: name.clone(),
                token: format!("token-{id}-{}", state.next_nonce),
                status: Status::Pending,
                error: None,
            });
            id
        })
        .collect();
    state.orders.push(Order {
        names,
        authorizations,
        certificate: None,
        finalized: false,
    });
    let id = state.orders.len() - 1;
    let location = format!("{}/order/{id}", state.base);
    let body = order_json(state, id);
    json_reply(state, StatusCode::CREATED, body, Some(location))
}

fn order_json(state: &State, id: usize) -> Value {
    let order = &state.orders[id];
    let status = state.order_status(order);
    let mut body = json!({
        "status": status.as_str(),
        "identifiers": order.names.iter().map(|n| json!({ "type": "dns", "value": n })).collect::<Vec<_>>(),
        "authorizations": order.authorizations.iter().map(|a| format!("{}/authz/{a}", state.base)).collect::<Vec<_>>(),
        "finalize": format!("{}/finalize/{id}", state.base),
    });
    if let Some(certificate) = order.certificate {
        body["certificate"] = json!(format!("{}/cert/{certificate}", state.base));
    }
    if status == Status::Invalid {
        let error = order
            .authorizations
            .iter()
            .find_map(|a| state.authorizations[*a].error.clone());
        body["error"] = error.unwrap_or(Value::Null);
    }
    body
}

fn authorization_json(state: &State, id: usize) -> Value {
    let authorization = &state.authorizations[id];
    let mut challenge = json!({
        "type": "http-01",
        "url": format!("{}/chall/{id}", state.base),
        "token": authorization.token,
        "status": authorization.status.as_str(),
    });
    if let Some(error) = &authorization.error {
        challenge["error"] = error.clone();
    }
    json!({
        "identifier": { "type": "dns", "value": authorization.name },
        "status": authorization.status.as_str(),
        "challenges": [challenge],
    })
}

/// Marks the challenge as being checked and fetches it in the background.
fn start_validation(state: &mut State, id: usize, shared: Shared) -> Response {
    let authorization = &state.authorizations[id];
    if authorization.status == Status::Pending {
        let name = authorization.name.clone();
        let token = authorization.token.clone();
        let expected = format!(
            "{token}.{}",
            state.accounts[authorization.account].thumbprint
        );
        let target = state.take_target();
        state.authorizations[id].status = Status::Processing;
        tokio::spawn(async move {
            let ok = match target {
                Some(target) => {
                    fetch(target, &name, &token).await.as_deref() == Some(expected.as_str())
                }
                None => false,
            };
            let mut state = shared
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(target) = target {
                state.validations.push(Validation {
                    name: name.clone(),
                    target,
                    ok,
                });
            }
            let authorization = &mut state.authorizations[id];
            if ok {
                authorization.status = Status::Valid;
            } else {
                authorization.status = Status::Invalid;
                authorization.error = Some(json!({
                    "type": format!("{PROBLEM}unauthorized"),
                    "detail": format!("the key authorization for {name} was not served"),
                    "status": 403,
                }));
            }
        });
    }
    let challenge = authorization_json(state, id)["challenges"][0].clone();
    json_reply(state, StatusCode::OK, challenge, None)
}

/// `GET /.well-known/acme-challenge/<token>` on `target` with `Host: name`;
/// the body when the answer is a 200.
async fn fetch(target: SocketAddr, name: &str, token: &str) -> Option<String> {
    let exchange = async {
        let mut stream = TcpStream::connect(target).await.ok()?;
        stream.set_nodelay(true).ok()?;
        let request = format!(
            "GET /.well-known/acme-challenge/{token} HTTP/1.1\r\nHost: {name}\r\nConnection: close\r\n\r\n"
        );
        stream.write_all(request.as_bytes()).await.ok()?;
        let mut answer = Vec::new();
        stream.read_to_end(&mut answer).await.ok()?;
        let text = String::from_utf8(answer).ok()?;
        let (head, body) = text.split_once("\r\n\r\n")?;
        head.starts_with("HTTP/1.1 200")
            .then(|| body.trim().to_string())
    };
    tokio::time::timeout(Duration::from_secs(10), exchange)
        .await
        .ok()
        .flatten()
}

fn finalize(state: &mut State, id: usize, signed: &Signed) -> Response {
    if state.order_status(&state.orders[id]) != Status::Ready {
        return problem(
            state,
            StatusCode::FORBIDDEN,
            "orderNotReady",
            "the order is not ready",
        );
    }
    let csr = signed
        .payload
        .get("csr")
        .and_then(Value::as_str)
        .and_then(b64);
    let spec = LeafSpec::new(&[]).expiring_in_days(state.lifetime_days);
    let Some(chain) = csr.and_then(|der| state.issuer.sign_csr(&der, &spec).ok()) else {
        return problem(
            state,
            StatusCode::BAD_REQUEST,
            "badCSR",
            "the CSR cannot be signed",
        );
    };
    state.certificates.push(chain);
    let order = &mut state.orders[id];
    order.finalized = true;
    order.certificate = Some(state.certificates.len() - 1);
    let body = order_json(state, id);
    json_reply(state, StatusCode::OK, body, None)
}

fn json_reply(
    state: &mut State,
    status: StatusCode,
    body: Value,
    location: Option<String>,
) -> Response {
    let mut response = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .header("replay-nonce", state.nonce());
    if let Some(location) = location {
        response = response.header(header::LOCATION, location);
    }
    let body = if body.is_null() {
        String::new()
    } else {
        body.to_string()
    };
    response.body(Body::from(body)).expect("a valid response")
}

fn problem(state: &mut State, status: StatusCode, kind: &str, detail: &str) -> Response {
    let body = json!({
        "type": format!("{PROBLEM}{kind}"),
        "detail": detail,
        "status": status.as_u16(),
    });
    let mut response = json_reply(state, status, body, None);
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/problem+json"),
    );
    response
}
