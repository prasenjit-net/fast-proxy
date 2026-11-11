//! The admin plane's protection, shared by its middleware and handlers: whether
//! tokens are required, whether the channel is safe for key material, the
//! `Host` allow-list, the failure throttle, WebSocket tickets and the live
//! token set.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};

use fast_proxy_auth::{FailureThrottle, TicketStore, TokenCache};
use fast_proxy_model::TokenId;

use crate::host_allow_list::HostAllowList;
use crate::AppError;

/// Failures from one address tolerated per window before it is refused.
const THROTTLE_MAX_FAILURES: usize = 10;
const THROTTLE_WINDOW_MS: i64 = 60_000;
/// How many addresses the throttle remembers.
const THROTTLE_ADDRESSES: usize = 10_000;
/// A WebSocket ticket lives this long and works once.
pub const TICKET_LIFETIME_MS: i64 = 30_000;
const TICKETS_OUTSTANDING: usize = 1_024;
/// How often one token's use is noted (`last_used_at` is coarse on purpose).
const TOKEN_USE_INTERVAL_MS: i64 = 60_000;
/// Audit entries for requests nobody authenticated (bad tokens, foreign `Host`
/// names) per minute for the whole process, so a flood cannot grow the log.
const UNAUTHENTICATED_AUDIT_PER_MINUTE: u32 = 60;
const MINUTE_MS: i64 = 60_000;

/// How the admin plane is configured to protect itself.
#[derive(Debug, Clone)]
pub struct SecuritySettings {
    /// Whether API calls and the WebSocket need a token.
    pub auth_required: bool,
    /// Whether the channel is safe for private keys: loopback, or TLS.
    pub channel_protected: bool,
    pub listen: SocketAddr,
    /// Extra names accepted in the `Host` header.
    pub allowed_hosts: Vec<String>,
    /// The stored certificate the admin plane serves, when it serves TLS.
    pub admin_certificate: Option<String>,
}

/// The protection state of one running admin plane.
pub struct AdminSecurity {
    pub(crate) auth_required: bool,
    pub(crate) channel_protected: bool,
    admin_certificate: Option<String>,
    hosts: HostAllowList,
    pub(crate) throttle: FailureThrottle,
    pub(crate) tickets: TicketStore,
    pub(crate) tokens: Arc<TokenCache>,
    token_use: Mutex<HashMap<TokenId, i64>>,
    unauthenticated_audit: Mutex<(i64, u32)>,
}

impl AdminSecurity {
    pub fn new(settings: &SecuritySettings, tokens: Arc<TokenCache>) -> Self {
        Self {
            auth_required: settings.auth_required,
            channel_protected: settings.channel_protected,
            admin_certificate: settings.admin_certificate.clone(),
            hosts: HostAllowList::new(settings.listen, &settings.allowed_hosts),
            throttle: FailureThrottle::new(
                THROTTLE_MAX_FAILURES,
                THROTTLE_WINDOW_MS,
                THROTTLE_ADDRESSES,
            ),
            tickets: TicketStore::new(TICKET_LIFETIME_MS, TICKETS_OUTSTANDING),
            tokens,
            token_use: Mutex::default(),
            unauthenticated_audit: Mutex::new((0, 0)),
        }
    }

    /// Refuses a request that carries private key material unless the channel
    /// is protected: loopback, or the admin plane served over TLS.
    pub(crate) fn require_protected_channel(&self) -> Result<(), AppError> {
        if self.channel_protected {
            return Ok(());
        }
        Err(AppError::Forbidden(
            "private keys are accepted only over a protected channel (loopback, or the admin \
             plane served over TLS); run the request on the machine the proxy runs on"
                .into(),
        ))
    }

    /// Whether `id`'s use should be noted now: at most once a minute per token,
    /// so authenticating does not turn every request into a write.
    pub(crate) fn token_use_due(&self, id: TokenId, now_ms: i64) -> bool {
        let mut seen = self
            .token_use
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match seen.get(&id) {
            Some(last) if now_ms - last < TOKEN_USE_INTERVAL_MS => false,
            _ => {
                seen.insert(id, now_ms);
                true
            }
        }
    }

    /// Whether one more audit entry for an unauthenticated request may be
    /// written in the current minute (a fixed window over the whole process).
    pub(crate) fn unauthenticated_audit_allowed(&self, now_ms: i64) -> bool {
        let mut window = self
            .unauthenticated_audit
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if now_ms - window.0 >= MINUTE_MS {
            *window = (now_ms, 0);
        }
        if window.1 >= UNAUTHENTICATED_AUDIT_PER_MINUTE {
            return false;
        }
        window.1 += 1;
        true
    }

    /// The name of the certificate the admin plane is serving, if it serves TLS.
    pub(crate) fn admin_certificate(&self) -> Option<&str> {
        self.admin_certificate.as_deref()
    }

    pub(crate) fn allows_host(&self, host_header: &str) -> bool {
        self.hosts.allows(host_header)
    }

    /// `required` or `off`, as the status endpoint reports it.
    pub(crate) fn auth_mode(&self) -> &'static str {
        if self.auth_required {
            "required"
        } else {
            "off"
        }
    }
}
