use std::fmt;

use crate::FieldError;

const MAX_TOKEN_NAME_LEN: usize = 63;

/// What an admin token may do. The permission table is [`Role::permits`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// Everything.
    Admin,
    /// Every read except key material; no writes, no token or audit-export management.
    Viewer,
}

/// The kind of operation an admin route needs; every route declares one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    /// Reading configuration, status and history.
    Read,
    /// Changing configuration.
    Write,
    /// A request that carries private key material.
    Keys,
    /// Creating, listing and revoking tokens.
    Tokens,
    /// Exporting the audit log in bulk.
    AuditExport,
}

impl Role {
    pub const ALL: [Role; 2] = [Role::Admin, Role::Viewer];

    /// The stored and API spelling: `admin` or `viewer`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Admin => "admin",
            Self::Viewer => "viewer",
        }
    }

    pub fn parse(raw: &str) -> Result<Self, FieldError> {
        match raw {
            "admin" => Ok(Self::Admin),
            "viewer" => Ok(Self::Viewer),
            _ => Err(FieldError::new("role", "must be admin or viewer")),
        }
    }

    /// The whole permission table: whether this role may perform `action`.
    pub fn permits(self, action: Action) -> bool {
        match (self, action) {
            (Self::Admin, _) => true,
            (Self::Viewer, Action::Read) => true,
            (Self::Viewer, Action::Write | Action::Keys | Action::Tokens | Action::AuditExport) => {
                false
            }
        }
    }
}

impl Action {
    pub const ALL: [Action; 5] = [
        Action::Read,
        Action::Write,
        Action::Keys,
        Action::Tokens,
        Action::AuditExport,
    ];
}

/// The database id of an admin token; it is the public half of `fp_<id>.<secret>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TokenId(pub i64);

/// A token's unique, human-readable name: 1 to 63 printable characters, with
/// no leading or trailing space. It appears in the audit log.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TokenName(String);

impl TokenName {
    pub fn parse(raw: &str) -> Result<Self, FieldError> {
        let invalid = |message: &str| Err(FieldError::new("name", message));
        if raw.is_empty() || raw.chars().count() > MAX_TOKEN_NAME_LEN {
            return invalid("must be 1 to 63 characters");
        }
        if raw.chars().any(char::is_control) {
            return invalid("must not contain control characters");
        }
        if raw.trim() != raw {
            return invalid("must not start or end with a space");
        }
        Ok(Self(raw.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TokenName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_permission_table_is_exactly_this() {
        let allowed = |role: Role| -> Vec<Action> {
            Action::ALL
                .into_iter()
                .filter(|action| role.permits(*action))
                .collect()
        };
        assert_eq!(allowed(Role::Admin), Action::ALL.to_vec());
        assert_eq!(allowed(Role::Viewer), vec![Action::Read]);
    }

    #[test]
    fn roles_round_trip_and_reject_unknown_spellings() {
        for role in Role::ALL {
            assert_eq!(Role::parse(role.as_str()).unwrap(), role);
        }
        for bad in ["", "Admin", "root", "viewer "] {
            assert_eq!(Role::parse(bad).unwrap_err().field, "role", "{bad:?}");
        }
    }

    #[test]
    fn token_names_accept_the_documented_range() {
        for name in ["a", "ci deploy", "Ünïcode ok", &"a".repeat(63)] {
            assert_eq!(TokenName::parse(name).unwrap().as_str(), name);
        }
    }

    #[test]
    fn token_names_reject_everything_else() {
        let too_long = "a".repeat(64);
        for name in ["", " a", "a ", "a\nb", "a\u{7}", too_long.as_str()] {
            assert_eq!(
                TokenName::parse(name).unwrap_err().field,
                "name",
                "{name:?}"
            );
        }
    }
}
