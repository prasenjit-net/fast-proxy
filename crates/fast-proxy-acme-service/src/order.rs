//! One ACME order, end to end (section 4.1 of the M4 plan): account, order,
//! publish the challenges, wait for other instances, self-check, ask the CA,
//! finalize with a fresh key, store. Each step's outcome is fed to the pure
//! [`OrderStep`] machine, so whoever cleans up after a failure knows how far
//! it got.

use fast_proxy_acme::{AcmeClient, AcmeError, AcmeSession, HttpChallenge, OrderEvent, OrderStep};
use fast_proxy_config_service::{
    AcmeAccountCredentials, ConfigService, IssuedCertificate, StoredAcmeCertificate,
};
use fast_proxy_model::{AcmeChallenge, AcmeToken, SubsystemId};
use fast_proxy_store::StoreError;
use fast_proxy_tls::generate_order_key;

use crate::{self_check, RunnerSettings};

/// How long a published challenge stays answerable.
const CHALLENGE_LIFETIME_MS: i64 = 3_600_000;

/// What one order needs from the runner.
pub(crate) struct OrderContext<'a> {
    pub service: &'a ConfigService,
    pub client: &'a AcmeClient,
    pub settings: &'a RunnerSettings,
    /// The account session, opened on the first order and kept.
    pub session: &'a mut Option<AcmeSession>,
}

/// Drives the order for `names` of `subsystem`, advancing `step` as it goes.
pub(crate) async fn drive(
    context: OrderContext<'_>,
    subsystem: SubsystemId,
    names: &[String],
    step: &mut OrderStep,
) -> Result<StoredAcmeCertificate, AcmeError> {
    let OrderContext {
        service,
        client,
        settings,
        session,
    } = context;
    let session = open_session(service, client, settings, session).await?;
    let mut order = session.new_order(names).await?;
    advance(step, OrderEvent::OrderCreated)?;

    let now = (settings.clock)();
    let published = order
        .challenges()
        .iter()
        .map(|challenge| to_stored(subsystem, challenge, now))
        .collect::<Result<Vec<_>, _>>()?;
    service
        .publish_acme_challenges(subsystem, published)
        .await
        .map_err(store_error)?;
    advance(step, OrderEvent::ChallengesPublished)?;

    tokio::time::sleep(settings.propagation_wait).await;
    advance(step, OrderEvent::PropagationElapsed)?;

    self_check::verify(settings.self_check, order.challenges()).await?;
    advance(step, OrderEvent::SelfCheckPassed)?;

    order.set_ready().await?;
    advance(step, OrderEvent::ValidationRequested)?;
    order.wait_ready(settings.ca_timeout).await?;
    advance(step, OrderEvent::Validated)?;

    let key = generate_order_key(names, settings.key_type).map_err(AcmeError::transient)?;
    let chain_pem = order.finalize(&key.csr_der, settings.ca_timeout).await?;
    advance(step, OrderEvent::CertificateAccepted)?;

    let stored = service
        .store_acme_certificate(IssuedCertificate {
            subsystem,
            chain_pem,
            key: key.key,
            names: names.to_vec(),
        })
        .await
        .map_err(store_error)?;
    advance(step, OrderEvent::CertificateStored)?;
    Ok(stored.value)
}

/// The kept session, or the stored account restored, or a new one registered
/// (and sealed into the database) on the very first order.
async fn open_session<'s>(
    service: &ConfigService,
    client: &AcmeClient,
    settings: &RunnerSettings,
    session: &'s mut Option<AcmeSession>,
) -> Result<&'s AcmeSession, AcmeError> {
    if session.is_none() {
        let opened = match service
            .acme_account(client.directory())
            .await
            .map_err(store_error)?
        {
            Some(account) => client.restore(account.credentials.expose()).await?,
            None => {
                let (opened, credentials) = client.register(&settings.email).await?;
                service
                    .save_acme_account(
                        client.directory(),
                        &settings.email,
                        &AcmeAccountCredentials::new(credentials),
                    )
                    .await
                    .map_err(store_error)?;
                opened
            }
        };
        *session = Some(opened);
    }
    session
        .as_ref()
        .ok_or_else(|| AcmeError::transient("no ACME account session"))
}

fn to_stored(
    subsystem: SubsystemId,
    challenge: &HttpChallenge,
    now_ms: i64,
) -> Result<AcmeChallenge, AcmeError> {
    let token = AcmeToken::parse(&challenge.token)
        .map_err(|_| AcmeError::transient("the CA sent a challenge token that is not base64url"))?;
    AcmeChallenge::new(
        token,
        subsystem,
        vec![challenge.name.to_ascii_lowercase()],
        challenge.key_authorization.clone(),
        now_ms.saturating_add(CHALLENGE_LIFETIME_MS),
    )
    .map_err(|err| AcmeError::transient(format!("the CA sent an unusable challenge: {err}")))
}

fn advance(step: &mut OrderStep, event: OrderEvent) -> Result<(), AcmeError> {
    *step = step
        .next(event)
        .map_err(|err| AcmeError::transient(err.to_string()))?;
    Ok(())
}

/// A storage failure as an order failure; internal detail stays in the log.
fn store_error(err: StoreError) -> AcmeError {
    match err {
        StoreError::Internal(detail) => {
            tracing::warn!("acme: a storage step failed: {detail}");
            AcmeError::transient("the database could not be written")
        }
        other => AcmeError::transient(other.to_string()),
    }
}
