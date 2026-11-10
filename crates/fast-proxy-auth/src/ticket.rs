use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use ring::rand::{SecureRandom, SystemRandom};
use thiserror::Error;

use crate::principal::Principal;

const TICKET_BYTES: usize = 16;

/// Why a ticket could not be issued.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum TicketError {
    /// Too many unredeemed tickets are outstanding.
    #[error("too many outstanding tickets")]
    Full,
    #[error("the system random number generator failed")]
    Random,
}

/// Single-use, short-lived tickets that let a browser open the WebSocket (it
/// cannot set an `Authorization` header). A ticket carries who asked for it, is
/// kept in memory on the issuing instance only, and the number outstanding is
/// bounded. The clock is a parameter.
pub struct TicketStore {
    lifetime_ms: i64,
    capacity: usize,
    issued: Mutex<HashMap<String, (Principal, i64)>>,
}

impl TicketStore {
    pub fn new(lifetime_ms: i64, capacity: usize) -> Self {
        Self {
            lifetime_ms,
            capacity: capacity.max(1),
            issued: Mutex::default(),
        }
    }

    /// Issues a ticket for `principal`, valid for the lifetime from `now_ms`.
    pub fn issue(&self, principal: Principal, now_ms: i64) -> Result<String, TicketError> {
        let mut bytes = [0u8; TICKET_BYTES];
        SystemRandom::new()
            .fill(&mut bytes)
            .map_err(|_| TicketError::Random)?;
        let ticket = URL_SAFE_NO_PAD.encode(bytes);
        let mut issued = self.lock();
        if issued.len() >= self.capacity {
            issued.retain(|_, (_, expires)| *expires > now_ms);
        }
        if issued.len() >= self.capacity {
            return Err(TicketError::Full);
        }
        issued.insert(ticket.clone(), (principal, now_ms + self.lifetime_ms));
        Ok(ticket)
    }

    /// Uses a ticket: returns its principal once, and never again. An unknown
    /// or expired ticket gives `None`.
    pub fn redeem(&self, ticket: &str, now_ms: i64) -> Option<Principal> {
        let (principal, expires) = self.lock().remove(ticket)?;
        (expires > now_ms).then_some(principal)
    }

    /// How many tickets are outstanding (expired ones included until purged).
    pub fn outstanding(&self) -> usize {
        self.lock().len()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, (Principal, i64)>> {
        self.issued.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use fast_proxy_model::{Role, TokenId};

    use super::*;

    fn principal(name: &str, role: Role) -> Principal {
        Principal {
            token_id: TokenId(1),
            name: name.into(),
            role,
            expires_at_ms: None,
        }
    }

    #[test]
    fn a_ticket_carries_its_principal_and_works_once() {
        let store = TicketStore::new(30_000, 10);
        let ticket = store.issue(principal("ann", Role::Viewer), 0).unwrap();
        assert_eq!(
            store.redeem(&ticket, 1_000),
            Some(principal("ann", Role::Viewer))
        );
        assert_eq!(store.redeem(&ticket, 1_000), None, "second use");
        assert_eq!(store.outstanding(), 0);
    }

    #[test]
    fn a_ticket_expires_at_the_exact_millisecond() {
        let store = TicketStore::new(30_000, 10);
        let early = store.issue(principal("ann", Role::Admin), 0).unwrap();
        let late = store.issue(principal("ann", Role::Admin), 0).unwrap();
        assert!(store.redeem(&early, 29_999).is_some());
        assert_eq!(store.redeem(&late, 30_000), None);
    }

    #[test]
    fn an_expired_ticket_is_consumed_too() {
        let store = TicketStore::new(10, 10);
        let ticket = store.issue(principal("ann", Role::Admin), 0).unwrap();
        assert_eq!(store.redeem(&ticket, 100), None);
        assert_eq!(store.outstanding(), 0);
    }

    #[test]
    fn unknown_tickets_are_refused() {
        let store = TicketStore::new(30_000, 10);
        assert_eq!(store.redeem("", 0), None);
        assert_eq!(store.redeem("guess", 0), None);
    }

    #[test]
    fn tickets_are_unique_and_unguessable_in_shape() {
        let store = TicketStore::new(30_000, 10);
        let a = store.issue(principal("a", Role::Admin), 0).unwrap();
        let b = store.issue(principal("a", Role::Admin), 0).unwrap();
        assert_ne!(a, b);
        assert_eq!(a.len(), 22, "128 bits, base64url");
    }

    #[test]
    fn the_number_outstanding_is_bounded_but_expired_tickets_make_room() {
        let store = TicketStore::new(1_000, 3);
        for _ in 0..3 {
            store.issue(principal("a", Role::Admin), 0).unwrap();
        }
        assert_eq!(
            store.issue(principal("a", Role::Admin), 500),
            Err(TicketError::Full)
        );
        assert!(store.issue(principal("a", Role::Admin), 1_000).is_ok());
        assert!(store.outstanding() <= 3);
    }
}
