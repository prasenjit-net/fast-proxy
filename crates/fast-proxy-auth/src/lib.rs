//! Admin authentication for fast-proxy. No HTTP and no database: callers pass
//! the clock as a parameter and map rows and requests in and out.
//!
//! Secrets exist in plaintext only at creation. What is stored and compared is
//! the SHA-256 of a 256-bit random secret, compared in constant time.

mod principal;
mod secret;
mod throttle;
mod ticket;
mod token;
mod token_set;

pub use principal::Principal;
pub use secret::{
    bypass_header_matches, hash_secret, secret_matches, GeneratedSecret, SecretError,
};
pub use throttle::{FailureThrottle, Throttled};
pub use ticket::{TicketError, TicketStore};
pub use token::{ParsedToken, PlaintextToken, TokenError};
pub use token_set::{AuthError, TokenCache, TokenRecord, TokenSet};
