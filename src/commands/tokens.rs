//! Admin tokens managed directly on the database file: the way back in when
//! every token is lost, and a way to create one before the proxy has ever
//! started. It works while the proxy runs (the revision it writes moves the
//! cluster generation, which the proxy's sync poll follows), writes an audit
//! entry with the actor `cli`, and respects the last-admin protection.

use std::fmt::Write as _;
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use fast_proxy_auth::{GeneratedSecret, PlaintextToken};
use fast_proxy_model::{Role, TokenId, TokenName};
use fast_proxy_store::repository::audit::{self, AuditOutcome, NewAuditEntry};
use fast_proxy_store::repository::tokens::{self, NewToken, TokenRow};
use fast_proxy_store::{revision, Connection, Database, Mutation, StoreError};

use crate::cli::TokenCreate;
use crate::commands::failure::{Failure, Outcome};

const DAY_MS: i64 = 86_400_000;
const ACTOR: &str = "cli";

/// Opens the database for writing, which must already exist (a typo in the path
/// must not create an empty database that the proxy then ignores).
fn open(path: &Path) -> Result<Database, Failure> {
    if !path.exists() {
        return Err(Failure::Usage(format!(
            "no database at {}; pass --database, or start the proxy once to create it",
            path.display()
        )));
    }
    Database::open(path)
        .map_err(|err| Failure::Refused(format!("cannot open {}: {err}", path.display())))
}

fn store_failure(err: StoreError) -> Failure {
    match err {
        StoreError::Conflict(message) => Failure::Refused(message),
        StoreError::Invalid(fields) => Failure::Usage(
            fields
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; "),
        ),
        StoreError::NotFound(what) => Failure::Refused(format!("{what} not found")),
        other => Failure::Refused(other.to_string()),
    }
}

/// An audit entry for something the command line did.
fn entry(action: &str, target: &str, summary: &str, version: u64, now_ms: i64) -> NewAuditEntry {
    NewAuditEntry {
        at_ms: now_ms,
        instance: ACTOR.into(),
        actor: ACTOR.into(),
        role: "system".into(),
        source: "local".into(),
        request_id: String::new(),
        action: action.into(),
        target: target.into(),
        outcome: AuditOutcome::Ok,
        detail: serde_json::json!({ "summary": summary }).to_string(),
        revision: Some(version),
    }
}

/// Writes one change as `cli`: the mutation, its audit entry and its revision
/// commit together.
fn write(
    db: &mut Database,
    action: &str,
    target: &str,
    summary: String,
    now_ms: i64,
    mutate: impl FnOnce(&Connection) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    db.write_as(
        ACTOR,
        |conn| {
            mutate(conn)?;
            let version = revision::current_version(conn)? + 1;
            audit::append(conn, &entry(action, target, &summary, version, now_ms))?;
            Ok(Mutation { value: (), summary })
        },
        |_| Ok(()),
    )
    .map(|_| ())
}

pub fn create(database: &Path, args: &TokenCreate, now_ms: i64) -> Outcome {
    let name = TokenName::parse(&args.name).map_err(|err| Failure::Usage(err.to_string()))?;
    let role = Role::parse(&args.role).map_err(|err| Failure::Usage(err.to_string()))?;
    if args.expires_days == Some(0) {
        return Err(Failure::Usage("--expires-days must be at least 1".into()));
    }
    let mut db = open(database)?;
    let secret = GeneratedSecret::generate().map_err(|err| Failure::Refused(err.to_string()))?;
    let new = NewToken {
        id: None,
        name: name.clone(),
        role,
        secret_hash: secret.hash().clone(),
        created_by: ACTOR.into(),
        expires_at_ms: args
            .expires_days
            .map(|days| now_ms + i64::from(days) * DAY_MS),
    };
    let created = std::cell::Cell::new(TokenId(0));
    write(
        &mut db,
        "token.create",
        name.as_str(),
        format!("token created: {} ({})", name.as_str(), role.as_str()),
        now_ms,
        |conn| {
            created.set(tokens::create(conn, &new, now_ms)?);
            Ok(())
        },
    )
    .map_err(store_failure)?;
    let token = PlaintextToken::new(created.get(), &secret);
    Ok(format!(
        "created the {} token \"{}\"; this is the only time it is shown, store it now:\n{}",
        role.as_str(),
        name.as_str(),
        token.reveal()
    ))
}

pub fn list(database: &Path) -> Outcome {
    let db = open(database)?;
    let rows = db.read(tokens::list).map_err(store_failure)?;
    if rows.is_empty() {
        return Ok("no tokens".to_string());
    }
    let mut out = format!(
        "{:<4} {:<24} {:<7} {:<10} {}\n",
        "ID", "NAME", "ROLE", "STATE", "LAST USED"
    );
    for row in rows {
        let _ = writeln!(
            out,
            "{:<4} {:<24} {:<7} {:<10} {}",
            row.id.0,
            row.name.as_str(),
            row.role.as_str(),
            state_of(&row),
            row.last_used_at_ms.map_or("never".to_string(), time_text)
        );
    }
    Ok(out.trim_end().to_string())
}

pub fn revoke(database: &Path, token: &str, now_ms: i64) -> Outcome {
    let mut db = open(database)?;
    let rows = db.read(tokens::list).map_err(store_failure)?;
    let target = find(&rows, token)
        .ok_or_else(|| Failure::Refused(format!("no token \"{token}\" (id or name)")))?;
    let (id, name) = (target.id, target.name.as_str().to_string());
    write(
        &mut db,
        "token.revoke",
        &name,
        format!("token revoked: {name}"),
        now_ms,
        |conn| tokens::revoke(conn, id, now_ms),
    )
    .map_err(store_failure)?;
    Ok(format!("revoked the token \"{name}\""))
}

fn find<'a>(rows: &'a [TokenRow], wanted: &str) -> Option<&'a TokenRow> {
    let id = wanted.parse::<i64>().ok();
    rows.iter()
        .find(|row| row.name.as_str() == wanted || id == Some(row.id.0))
}

fn state_of(row: &TokenRow) -> &'static str {
    if row.revoked_at_ms.is_some() {
        "revoked"
    } else {
        "active"
    }
}

fn time_text(ms: i64) -> String {
    let at = UNIX_EPOCH + Duration::from_millis(u64::try_from(ms).unwrap_or(0));
    humantime::format_rfc3339_seconds(at).to_string()
}

#[cfg(test)]
mod tests {
    use fast_proxy_store::repository::audit::AuditQuery;

    use super::*;

    const NOW: i64 = 1_800_000_000_000;

    fn database() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fast-proxy.db");
        drop(Database::open(&path).unwrap());
        (dir, path)
    }

    fn args(name: &str, role: &str) -> TokenCreate {
        TokenCreate {
            name: name.into(),
            role: role.into(),
            expires_days: None,
        }
    }

    fn token_of(text: &str) -> String {
        text.lines().last().unwrap().to_string()
    }

    #[test]
    fn a_created_token_is_printed_once_and_stored_only_as_a_hash() {
        let (_dir, path) = database();
        let text = create(&path, &args("rescue", "admin"), NOW).unwrap();
        let token = token_of(&text);
        assert!(token.starts_with("fp_1."), "{token}");
        let secret = token.rsplit('.').next().unwrap();

        let listed = list(&path).unwrap();
        assert!(listed.contains("rescue") && listed.contains("admin") && listed.contains("active"));
        assert!(!listed.contains(secret));
        let raw = std::fs::read(&path).unwrap();
        assert!(!raw.windows(secret.len()).any(|w| w == secret.as_bytes()));
    }

    #[test]
    fn creating_writes_a_revision_and_an_audit_entry_as_cli() {
        let (_dir, path) = database();
        create(&path, &args("rescue", "viewer"), NOW).unwrap();
        let db = Database::open(&path).unwrap();
        assert_eq!(db.current_version().unwrap(), 1);
        let revision = db.read(|c| revision::get(c, 1)).unwrap();
        assert_eq!(revision.actor, "cli");
        let entries = db
            .read(|c| audit::list(c, &AuditQuery::default(), None, 10))
            .unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].entry.action, "token.create");
        assert_eq!(entries[0].entry.actor, "cli");
        assert_eq!(entries[0].entry.target, "rescue");
        assert_eq!(entries[0].entry.revision, Some(1));
    }

    #[test]
    fn expiry_in_days_is_stored_and_zero_is_refused() {
        let (_dir, path) = database();
        let short = TokenCreate {
            expires_days: Some(2),
            ..args("temp", "viewer")
        };
        create(&path, &short, NOW).unwrap();
        let db = Database::open(&path).unwrap();
        let rows = db.read(tokens::list).unwrap();
        assert_eq!(rows[0].expires_at_ms, Some(NOW + 2 * DAY_MS));
        let zero = TokenCreate {
            expires_days: Some(0),
            ..args("x", "viewer")
        };
        assert!(matches!(
            create(&path, &zero, NOW).unwrap_err(),
            Failure::Usage(_)
        ));
    }

    #[test]
    fn bad_names_roles_and_duplicates_are_refused() {
        let (_dir, path) = database();
        assert!(matches!(
            create(&path, &args("", "admin"), NOW).unwrap_err(),
            Failure::Usage(_)
        ));
        assert!(matches!(
            create(&path, &args("a", "root"), NOW).unwrap_err(),
            Failure::Usage(_)
        ));
        create(&path, &args("dup", "admin"), NOW).unwrap();
        let err = create(&path, &args("dup", "viewer"), NOW).unwrap_err();
        assert!(
            matches!(&err, Failure::Refused(m) if m.contains("\"dup\"")),
            "{err:?}"
        );
    }

    #[test]
    fn a_missing_database_is_a_usage_error_and_is_not_created() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("typo.db");
        for outcome in [
            create(&path, &args("x", "admin"), NOW),
            list(&path),
            revoke(&path, "x", NOW),
        ] {
            assert!(matches!(outcome.unwrap_err(), Failure::Usage(_)));
        }
        assert!(!path.exists());
    }

    #[test]
    fn the_last_admin_cannot_be_revoked_but_a_spare_can() {
        let (_dir, path) = database();
        create(&path, &args("only", "admin"), NOW).unwrap();
        let err = revoke(&path, "only", NOW).unwrap_err();
        assert!(
            matches!(&err, Failure::Refused(m) if m.contains("last usable admin")),
            "{err:?}"
        );
        create(&path, &args("spare", "admin"), NOW).unwrap();
        assert!(revoke(&path, "only", NOW).unwrap().contains("revoked"));
        assert!(list(&path).unwrap().contains("revoked"));
    }

    #[test]
    fn a_token_is_revoked_by_id_or_by_name_and_an_unknown_one_is_refused() {
        let (_dir, path) = database();
        create(&path, &args("a", "admin"), NOW).unwrap();
        create(&path, &args("b", "viewer"), NOW).unwrap();
        create(&path, &args("c", "viewer"), NOW).unwrap();
        assert!(revoke(&path, "b", NOW).is_ok());
        assert!(revoke(&path, "3", NOW).is_ok());
        assert!(matches!(
            revoke(&path, "nope", NOW).unwrap_err(),
            Failure::Refused(_)
        ));
        let db = Database::open(&path).unwrap();
        let entries = db
            .read(|c| {
                audit::list(
                    c,
                    &AuditQuery {
                        action: Some("token.revoke".into()),
                        ..AuditQuery::default()
                    },
                    None,
                    10,
                )
            })
            .unwrap();
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn listing_an_empty_database_says_so() {
        let (_dir, path) = database();
        assert_eq!(list(&path).unwrap(), "no tokens");
    }
}
