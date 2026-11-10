//! The admin plane's protection, shared by its middleware and handlers: whether
//! tokens are required, whether the channel is safe for key material, the
//! `Host` allow-list, the failure throttle, WebSocket tickets and the live
//! token set.

use std::net::SocketAddr;
use std::sync::Arc;

use fast_proxy_auth::{FailureThrottle, TicketStore, TokenCache};

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
