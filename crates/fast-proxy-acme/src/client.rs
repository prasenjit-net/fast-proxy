//! The ACME protocol client (K1, ADR 0024): a thin adapter over `instant-acme`,
//! so swapping the library touches this file only. It speaks for one directory
//! and one account; every error becomes an [`AcmeError`] whose kind decides the
//! backoff and whose detail is safe to store and show.

use std::path::PathBuf;
use std::time::Duration;

use fast_proxy_model::sanitize_error_detail;
use instant_acme::{
    Account, AccountCredentials, AuthorizationStatus, ChallengeType, Error, Identifier, NewAccount,
    NewOrder, Order, OrderStatus, Problem, RetryPolicy,
};

use crate::{classify_problem, FailureKind};

/// A failed protocol step: how to treat it and a sanitized, one-line detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcmeError {
    pub kind: FailureKind,
    pub detail: String,
}

impl AcmeError {
    /// A failure of our own (not the CA's), worth retrying.
    pub fn transient(detail: impl AsRef<str>) -> Self {
        Self {
            kind: FailureKind::Transient,
            detail: sanitize_error_detail(detail.as_ref()),
        }
    }

    fn from_problem(problem: &Problem) -> Self {
        let kind = classify_problem(problem.r#type.as_deref().unwrap_or(""), None);
        let short_type = problem
            .r#type
            .as_deref()
            .and_then(|urn| urn.rsplit(':').next())
            .unwrap_or("error");
        let detail = problem.detail.as_deref().unwrap_or("");
        Self {
            kind,
            detail: sanitize_error_detail(&format!("{short_type}: {detail}")),
        }
    }
}

impl std::fmt::Display for AcmeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.detail)
    }
}

impl std::error::Error for AcmeError {}

impl From<Error> for AcmeError {
    fn from(err: Error) -> Self {
        match &err {
            Error::Api(problem) => Self::from_problem(problem),
            // A network or parse failure: the message names no key, only what failed.
            other => Self::transient(other.to_string()),
        }
    }
}

/// One directory, reached with the platform roots or a given root (`directory_ca`).
#[derive(Debug, Clone)]
pub struct AcmeClient {
    directory: String,
    directory_ca: Option<PathBuf>,
}

/// An account the CA knows.
pub struct AcmeSession {
    account: Account,
}

/// An HTTP-01 challenge to publish: the name, the token and its key authorization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpChallenge {
    pub name: String,
    pub token: String,
    pub key_authorization: String,
}

/// An order with its pending HTTP-01 challenges.
pub struct PendingOrder {
    order: Order,
    challenges: Vec<HttpChallenge>,
}

impl AcmeClient {
    pub fn new(directory: impl Into<String>, directory_ca: Option<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
            directory_ca,
        }
    }

    /// The directory URL this client speaks to.
    pub fn directory(&self) -> &str {
        &self.directory
    }

    fn builder(&self) -> Result<instant_acme::AccountBuilder, AcmeError> {
        Ok(match &self.directory_ca {
            Some(path) => Account::builder_with_root(path)?,
            None => Account::builder()?,
        })
    }

    /// Registers a new account (agreeing to the CA's terms) and returns it with
    /// its serialized credentials, to be sealed and stored by the caller.
    pub async fn register(&self, email: &str) -> Result<(AcmeSession, Vec<u8>), AcmeError> {
        let contact = format!("mailto:{email}");
        let contacts: Vec<&str> = if email.is_empty() {
            Vec::new()
        } else {
            vec![contact.as_str()]
        };
        let (account, credentials) = self
            .builder()?
            .create(
                &NewAccount {
                    contact: &contacts,
                    terms_of_service_agreed: true,
                    only_return_existing: false,
                },
                self.directory.clone(),
                None,
            )
            .await?;
        let bytes = serde_json::to_vec(&credentials)
            .map_err(|_| AcmeError::transient("the account credentials cannot be serialized"))?;
        Ok((AcmeSession { account }, bytes))
    }

    /// Restores a stored account from its serialized credentials.
    pub async fn restore(&self, credentials: &[u8]) -> Result<AcmeSession, AcmeError> {
        let credentials: AccountCredentials = serde_json::from_slice(credentials)
            .map_err(|_| AcmeError::transient("the stored account credentials are not readable"))?;
        let account = self.builder()?.from_credentials(credentials).await?;
        Ok(AcmeSession { account })
    }
}

impl AcmeSession {
    /// Orders a certificate for `names` and collects each name's HTTP-01
    /// challenge. A name already authorized has no challenge to publish.
    pub async fn new_order(&self, names: &[String]) -> Result<PendingOrder, AcmeError> {
        let identifiers: Vec<Identifier> = names.iter().cloned().map(Identifier::Dns).collect();
        let mut order = self.account.new_order(&NewOrder::new(&identifiers)).await?;
        let mut challenges = Vec::new();
        let mut authorizations = order.authorizations();
        while let Some(handle) = authorizations.next().await {
            let mut authorization = handle?;
            if authorization.status != AuthorizationStatus::Pending {
                continue;
            }
            let name = authorization.identifier().to_string();
            let challenge = authorization
                .challenge(ChallengeType::Http01)
                .ok_or_else(|| AcmeError {
                    kind: FailureKind::Permanent,
                    detail: format!("the CA offers no http-01 challenge for {name}"),
                })?;
            challenges.push(HttpChallenge {
                name,
                token: challenge.token.clone(),
                key_authorization: challenge.key_authorization().as_str().to_string(),
            });
        }
        Ok(PendingOrder { order, challenges })
    }
}

impl PendingOrder {
    /// The challenges to publish before [`set_ready`](Self::set_ready).
    pub fn challenges(&self) -> &[HttpChallenge] {
        &self.challenges
    }

    /// Tells the CA every pending challenge can be checked now.
    pub async fn set_ready(&mut self) -> Result<(), AcmeError> {
        let mut authorizations = self.order.authorizations();
        while let Some(handle) = authorizations.next().await {
            let mut authorization = handle?;
            if authorization.status != AuthorizationStatus::Pending {
                continue;
            }
            if let Some(mut challenge) = authorization.challenge(ChallengeType::Http01) {
                challenge.set_ready().await?;
            }
        }
        Ok(())
    }

    /// Waits up to `timeout` for the CA to validate every name. An invalid
    /// order reports the problem of the first challenge that failed.
    pub async fn wait_ready(&mut self, timeout: Duration) -> Result<(), AcmeError> {
        let status = self.order.poll_ready(&retries(timeout)).await?;
        if status == OrderStatus::Ready {
            return Ok(());
        }
        Err(self.validation_problem().await)
    }

    /// Sends the CSR and waits up to `timeout` for the chain (PEM, leaf first).
    pub async fn finalize(
        &mut self,
        csr_der: &[u8],
        timeout: Duration,
    ) -> Result<String, AcmeError> {
        self.order.finalize_csr(csr_der).await?;
        Ok(self.order.poll_certificate(&retries(timeout)).await?)
    }

    /// Why validation failed, from the first challenge carrying an error.
    async fn validation_problem(&mut self) -> AcmeError {
        let mut authorizations = self.order.authorizations();
        while let Some(Ok(mut authorization)) = authorizations.next().await {
            if authorization.refresh().await.is_err() {
                continue;
            }
            let failed = authorization
                .challenges
                .iter()
                .find_map(|challenge| challenge.error.as_ref());
            if let Some(problem) = failed {
                return AcmeError::from_problem(problem);
            }
        }
        AcmeError::transient("the CA could not validate the order")
    }
}

fn retries(timeout: Duration) -> RetryPolicy {
    RetryPolicy::new()
        .initial_delay(Duration::from_millis(250))
        .backoff(1.5)
        .timeout(timeout)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn problem(r#type: &str, detail: &str) -> Problem {
        serde_json::from_value(serde_json::json!({
            "type": r#type,
            "detail": detail,
            "status": 400
        }))
        .unwrap()
    }

    #[test]
    fn a_ca_problem_keeps_its_kind_and_a_short_detail() {
        let err = AcmeError::from(Error::Api(problem(
            "urn:ietf:params:acme:error:rateLimited",
            "too many certificates\nfor this name",
        )));
        assert_eq!(err.kind, FailureKind::RateLimited { retry_at_ms: None });
        assert_eq!(
            err.detail,
            "rateLimited: too many certificates for this name"
        );

        let err = AcmeError::from(Error::Api(problem(
            "urn:ietf:params:acme:error:dns",
            "NXDOMAIN",
        )));
        assert_eq!(err.kind, FailureKind::Permanent);
        assert_eq!(err.to_string(), "dns: NXDOMAIN");
    }

    #[test]
    fn a_local_failure_is_transient() {
        let err = AcmeError::from(Error::Str("no nonce"));
        assert_eq!(err.kind, FailureKind::Transient);
        assert!(err.detail.contains("no nonce"));
    }

    #[test]
    fn stored_credentials_that_do_not_parse_are_refused_without_a_request() {
        let client = AcmeClient::new("https://127.0.0.1:1/dir", None);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let err = runtime.block_on(client.restore(b"not json")).err().unwrap();
        assert_eq!(err.kind, FailureKind::Transient);
        assert_eq!(client.directory(), "https://127.0.0.1:1/dir");
    }
}
