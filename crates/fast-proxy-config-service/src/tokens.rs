//! Admin token writes. Tokens go through [`ConfigService::apply`] like every
//! other change, so each is a revision (the configuration copy unchanged, which
//! bumps the cluster generation) with an audit entry naming who made it. The
//! in-memory [`TokenCache`] is rebuilt after every commit and every reload.
//!
//! Secrets exist in plaintext only in the value these methods return once; the
//! database holds the hash, and neither the summary nor the audit entry names
//! anything but the token's name and role.

use std::sync::Arc;

use fast_proxy_auth::{
    hash_secret, GeneratedSecret, ParsedToken, PlaintextToken, TokenCache, TokenRecord,
};
use fast_proxy_model::{Role, TokenId, TokenName};
use fast_proxy_store::repository::tokens::{self, NewToken, TokenCredential, TokenRow};
use fast_proxy_store::{Mutation, StoreError};

use crate::service::{now_ms, NOTHING_TO_DO};
use crate::{Applied, Audited, ChangeContext, ConfigService};

/// A seeded bootstrap secret shorter than this is refused: it must be as hard
/// to guess as a generated one (a generated secret is 43 characters).
const MIN_SEEDED_SECRET_LEN: usize = 32;

/// The name of the first admin token.
const BOOTSTRAP_NAME: &str = "bootstrap";

/// A token that was just created; the only time its secret is available.
#[derive(Debug)]
pub struct CreatedToken {
    pub id: TokenId,
    pub token: PlaintextToken,
}

/// What [`ConfigService::ensure_admin_token`] did.
#[derive(Debug)]
pub enum Bootstrap {
    /// A usable admin token already exists.
    NotNeeded,
    /// A token was generated; show it once.
    Created(PlaintextToken),
    /// The operator's own token (from the environment) was stored; nothing to show.
    Seeded,
}

impl ConfigService {
    /// The live token set, for the admin plane's authentication.
    pub fn token_cache(&self) -> Arc<TokenCache> {
        self.tokens.clone()
    }

    /// Creates a token. `expires_at_ms` of `None` never expires.
    pub async fn create_token(
        &self,
        context: &ChangeContext,
        name: &str,
        role: Role,
        expires_at_ms: Option<i64>,
    ) -> Result<Applied<CreatedToken>, StoreError> {
        let name = TokenName::parse(name)?;
        let secret = GeneratedSecret::generate().map_err(internal)?;
        let new = NewToken {
            id: None,
            name: name.clone(),
            role,
            secret_hash: secret.hash().clone(),
            created_by: context.actor.clone(),
            expires_at_ms,
        };
        let applied = self
            .apply(
                context,
                Audited::new("token.create", name.as_str()),
                move |conn| {
                    Ok(Mutation {
                        value: tokens::create(conn, &new, now_ms())?,
                        summary: format!("token created: {} ({})", name.as_str(), role.as_str()),
                    })
                },
            )
            .await?;
        Ok(Applied {
            value: CreatedToken {
                id: applied.value,
                token: PlaintextToken::new(applied.value, &secret),
            },
            version: applied.version,
        })
    }

    /// Revokes a token at once. The last usable admin token is a `Conflict`.
    pub async fn revoke_token(
        &self,
        context: &ChangeContext,
        id: TokenId,
    ) -> Result<Applied<()>, StoreError> {
        self.apply(
            context,
            Audited::new("token.revoke", format!("token:{}", id.0)),
            move |conn| {
                let name = tokens::find(conn, id)?.name;
                tokens::revoke(conn, id, now_ms())?;
                Ok(Mutation {
                    value: (),
                    summary: format!("token revoked: {}", name.as_str()),
                })
            },
        )
        .await
    }

    /// Every token, without secrets or hashes.
    pub async fn list_tokens(&self) -> Result<Vec<TokenRow>, StoreError> {
        Ok(self.read(tokens::list).await?.value)
    }

    /// Makes sure a usable admin token exists, so a fresh install is neither
    /// open nor locked out. With none, stores `seed` (a complete
    /// `fp_<id>.<secret>` the operator chose, for containers) or generates one.
    /// Several instances starting together on one database create only one.
    pub async fn ensure_admin_token(&self, seed: Option<&str>) -> Result<Bootstrap, StoreError> {
        let seeded = seed.map(parse_seed).transpose()?;
        let generated = match seeded {
            Some(_) => None,
            None => Some(GeneratedSecret::generate().map_err(internal)?),
        };
        let secret_hash = match (&seeded, &generated) {
            (Some((_, secret)), _) => hash_secret(secret),
            (None, Some(generated)) => generated.hash().clone(),
            (None, None) => return Err(internal("no secret to store")),
        };
        let seeded_id = seeded.as_ref().map(|(id, _)| *id);
        let context = ChangeContext::token(BOOTSTRAP_NAME, "system", "local", "");
        let result = self
            .apply(
                &context,
                Audited::new("token.bootstrap", BOOTSTRAP_NAME),
                move |conn| {
                    if tokens::usable_admin_count(conn, now_ms())? > 0 {
                        return Err(StoreError::Conflict(NOTHING_TO_DO.into()));
                    }
                    let name = unused_bootstrap_name(conn)?;
                    let new = NewToken {
                        id: seeded_id,
                        name: name.clone(),
                        role: Role::Admin,
                        secret_hash,
                        created_by: BOOTSTRAP_NAME.into(),
                        expires_at_ms: None,
                    };
                    Ok(Mutation {
                        value: tokens::create(conn, &new, now_ms())?,
                        summary: format!("admin token created: {}", name.as_str()),
                    })
                },
            )
            .await;
        match (result, generated) {
            (Ok(applied), Some(secret)) => Ok(Bootstrap::Created(PlaintextToken::new(
                applied.value,
                &secret,
            ))),
            (Ok(_), None) => Ok(Bootstrap::Seeded),
            (Err(StoreError::Conflict(message)), _) if message == NOTHING_TO_DO => {
                Ok(Bootstrap::NotNeeded)
            }
            (Err(err), _) => Err(err),
        }
    }

    /// Rebuilds the in-memory token set from the database.
    pub(crate) async fn refresh_tokens(&self) -> Result<(), StoreError> {
        let credentials = self.store.run(|db| db.read(tokens::credentials)).await?;
        self.tokens
            .replace(credentials.into_iter().map(into_record).collect());
        Ok(())
    }
}

fn parse_seed(seed: &str) -> Result<(TokenId, String), StoreError> {
    let invalid = |message: &str| StoreError::invalid_field("FAST_PROXY_BOOTSTRAP_TOKEN", message);
    let parsed =
        ParsedToken::parse(seed).map_err(|_| invalid("must have the form fp_<id>.<secret>"))?;
    if parsed.secret.len() < MIN_SEEDED_SECRET_LEN {
        return Err(invalid("the secret must be at least 32 characters"));
    }
    Ok((parsed.id, parsed.secret.to_string()))
}

/// `bootstrap`, or `bootstrap-<n>` when earlier bootstrap tokens (revoked or
/// expired) still hold the name.
fn unused_bootstrap_name(conn: &fast_proxy_store::Connection) -> Result<TokenName, StoreError> {
    let taken: Vec<String> = tokens::list(conn)?
        .into_iter()
        .map(|token| token.name.as_str().to_string())
        .collect();
    let name = std::iter::once(BOOTSTRAP_NAME.to_string())
        .chain((2..).map(|n| format!("{BOOTSTRAP_NAME}-{n}")))
        .find(|candidate| !taken.contains(candidate))
        .unwrap_or_else(|| BOOTSTRAP_NAME.to_string());
    Ok(TokenName::parse(&name)?)
}

fn into_record(credential: TokenCredential) -> TokenRecord {
    TokenRecord {
        id: credential.id,
        name: credential.name,
        role: credential.role,
        secret_hash: credential.secret_hash,
        expires_at_ms: credential.expires_at_ms,
        revoked: credential.revoked,
    }
}

fn internal(err: impl std::fmt::Display) -> StoreError {
    StoreError::Internal(err.to_string())
}
