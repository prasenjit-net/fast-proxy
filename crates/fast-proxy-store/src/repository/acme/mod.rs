//! ACME state (M4): the account, pending challenges, the renewal lease and the
//! per-subsystem status. None of it is `Config`: a revision never holds it and a
//! rollback never changes it. The account credentials are opaque ciphertext
//! here, like every key (this crate never decrypts).

pub mod account;
pub mod challenges;
pub mod lease;
pub mod status;

#[cfg(test)]
mod tests;
