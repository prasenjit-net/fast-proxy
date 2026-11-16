//! The server's memory: accounts, orders, authorizations and certificates.

use std::net::SocketAddr;

use fast_proxy_test_pki::TestCa;

/// One challenge fetch the server made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Validation {
    pub name: String,
    pub target: SocketAddr,
    /// The proxy answered with the expected key authorization.
    pub ok: bool,
}

pub(crate) struct Account {
    pub thumbprint: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Status {
    Pending,
    Processing,
    Ready,
    Valid,
    Invalid,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Processing => "processing",
            Self::Ready => "ready",
            Self::Valid => "valid",
            Self::Invalid => "invalid",
        }
    }
}

pub(crate) struct Authorization {
    pub account: usize,
    pub name: String,
    pub token: String,
    /// The authorization's status; the challenge's follows it.
    pub status: Status,
    pub error: Option<serde_json::Value>,
}

pub(crate) struct Order {
    pub names: Vec<String>,
    pub authorizations: Vec<usize>,
    pub certificate: Option<usize>,
    pub finalized: bool,
}

pub(crate) struct State {
    pub base: String,
    pub issuer: TestCa,
    pub next_nonce: u64,
    pub accounts: Vec<Account>,
    pub orders: Vec<Order>,
    pub authorizations: Vec<Authorization>,
    pub certificates: Vec<String>,
    pub validations: Vec<Validation>,
    pub targets: Vec<SocketAddr>,
    pub next_target: usize,
    pub rate_limited: bool,
    pub lifetime_days: i64,
}

impl State {
    pub fn new(base: String, issuer: TestCa) -> Self {
        Self {
            base,
            issuer,
            next_nonce: 0,
            accounts: Vec::new(),
            orders: Vec::new(),
            authorizations: Vec::new(),
            certificates: Vec::new(),
            validations: Vec::new(),
            targets: Vec::new(),
            next_target: 0,
            rate_limited: false,
            lifetime_days: 90,
        }
    }

    pub fn nonce(&mut self) -> String {
        self.next_nonce += 1;
        format!("nonce-{}", self.next_nonce)
    }

    /// An order's status, from its authorizations and whether it was finalized.
    pub fn order_status(&self, order: &Order) -> Status {
        if order.certificate.is_some() {
            return Status::Valid;
        }
        if order.finalized {
            return Status::Processing;
        }
        let statuses: Vec<Status> = order
            .authorizations
            .iter()
            .map(|id| self.authorizations[*id].status)
            .collect();
        if statuses.contains(&Status::Invalid) {
            Status::Invalid
        } else if statuses.iter().all(|s| *s == Status::Valid) {
            Status::Ready
        } else {
            Status::Pending
        }
    }

    /// The next fetch target, round robin.
    pub fn take_target(&mut self) -> Option<SocketAddr> {
        if self.targets.is_empty() {
            return None;
        }
        let target = self.targets[self.next_target % self.targets.len()];
        self.next_target += 1;
        Some(target)
    }
}
