use fast_proxy_model::{Role, TokenId};

/// Who is making an admin request: the authenticated token's identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal {
    pub token_id: TokenId,
    /// The token's name, which is what the audit log records.
    pub name: String,
    pub role: Role,
    /// When the token stops working, if it ever does (a ticket inherits it).
    pub expires_at_ms: Option<i64>,
}
