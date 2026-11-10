//! Maintenance scopes: at most one row per target (the whole proxy, a
//! subsystem or a route). Rows are removed with their subsystem or route.
//! Maintenance is not part of `Config`; a rollback keeps it (see
//! [`reinsert_surviving`]).

use fast_proxy_model::{
    BodyContentType, BypassHeader, BypassRules, Cidr, CustomBody, MaintenanceSettings,
    MaintenanceTarget, RouteId, SecretHash, SubsystemId,
};
use rusqlite::{params, Connection, OptionalExtension};

use super::ensure_subsystem;
use crate::StoreError;

/// A stored scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaintenanceRow {
    pub id: i64,
    pub target: MaintenanceTarget,
    pub settings: MaintenanceSettings,
}

/// Turns maintenance on for `target`, or replaces its settings. Invalid
/// settings are refused with the offending field; a missing target is `NotFound`.
pub fn set(
    conn: &Connection,
    target: MaintenanceTarget,
    settings: &MaintenanceSettings,
) -> Result<(), StoreError> {
    settings.validate()?;
    ensure_target_exists(conn, target)?;
    let (scope, subsystem_id, route_id) = target_columns(target);
    let (body, content_type) = settings.body.as_ref().map_or(("", ""), |body| {
        (body.text.as_str(), body.content_type.as_str())
    });
    let (header_name, header_hash) = settings.bypass.header.as_ref().map_or(("", None), |h| {
        (h.name.as_str(), Some(h.secret_hash.as_bytes()))
    });
    let cidrs = serde_json::to_string(
        &settings
            .bypass
            .cidrs
            .iter()
            .map(Cidr::to_cidr_string)
            .collect::<Vec<_>>(),
    )
    .map_err(|err| StoreError::Internal(err.to_string()))?;
    conn.execute(
        "DELETE FROM maintenance
         WHERE scope = ?1 AND COALESCE(subsystem_id, 0) = ?2 AND COALESCE(route_id, 0) = ?3",
        params![scope, subsystem_id.unwrap_or(0), route_id.unwrap_or(0)],
    )?;
    conn.execute(
        "INSERT INTO maintenance
           (scope, subsystem_id, route_id, message, body, content_type, retry_after_secs,
            bypass_cidrs, bypass_header, bypass_secret_hash, expires_at, set_by, set_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            scope,
            subsystem_id,
            route_id,
            settings.message,
            body,
            content_type,
            settings.retry_after_secs,
            cidrs,
            header_name,
            header_hash,
            settings.expires_at_ms,
            settings.set_by,
            settings.set_at_ms,
        ],
    )?;
    Ok(())
}

/// Turns maintenance off for `target`; returns whether a scope was active, so
/// clearing an already-cleared scope is a harmless no-op.
pub fn clear(conn: &Connection, target: MaintenanceTarget) -> Result<bool, StoreError> {
    let (scope, subsystem_id, route_id) = target_columns(target);
    let changed = conn.execute(
        "DELETE FROM maintenance
         WHERE scope = ?1 AND COALESCE(subsystem_id, 0) = ?2 AND COALESCE(route_id, 0) = ?3",
        params![scope, subsystem_id.unwrap_or(0), route_id.unwrap_or(0)],
    )?;
    Ok(changed > 0)
}

/// Every stored scope, oldest first. A row that no longer validates is
/// corruption and fails the call (`Internal`).
pub fn list(conn: &Connection) -> Result<Vec<MaintenanceRow>, StoreError> {
    let mut statement = conn.prepare(
        "SELECT id, scope, subsystem_id, route_id, message, body, content_type, retry_after_secs,
                bypass_cidrs, bypass_header, bypass_secret_hash, expires_at, set_by, set_at
         FROM maintenance ORDER BY id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(RawRow {
            id: row.get(0)?,
            scope: row.get(1)?,
            subsystem_id: row.get(2)?,
            route_id: row.get(3)?,
            message: row.get(4)?,
            body: row.get(5)?,
            content_type: row.get(6)?,
            retry_after_secs: row.get(7)?,
            bypass_cidrs: row.get(8)?,
            bypass_header: row.get(9)?,
            bypass_secret_hash: row.get(10)?,
            expires_at_ms: row.get(11)?,
            set_by: row.get(12)?,
            set_at_ms: row.get(13)?,
        })
    })?;
    rows.map(|row| row?.into_row()).collect()
}

/// Puts back scopes taken from [`list`] before a rollback replaced the
/// configuration. Ids are preserved by a rollback, so a scope survives when its
/// subsystem or route still exists, and goes with its target when it does not.
pub fn reinsert_surviving(conn: &Connection, rows: &[MaintenanceRow]) -> Result<(), StoreError> {
    for row in rows {
        match set(conn, row.target, &row.settings) {
            Ok(()) | Err(StoreError::NotFound(_)) => {}
            Err(other) => return Err(other),
        }
    }
    Ok(())
}

fn ensure_target_exists(conn: &Connection, target: MaintenanceTarget) -> Result<(), StoreError> {
    match target {
        MaintenanceTarget::Global => Ok(()),
        MaintenanceTarget::Subsystem(id) => ensure_subsystem(conn, id),
        MaintenanceTarget::Route(id) => {
            let found = conn
                .query_row("SELECT 1 FROM route WHERE id = ?1", [id.0], |_| Ok(()))
                .optional()?;
            found.ok_or_else(|| StoreError::NotFound(format!("route {}", id.0)))
        }
    }
}

fn target_columns(target: MaintenanceTarget) -> (&'static str, Option<i64>, Option<i64>) {
    match target {
        MaintenanceTarget::Global => ("global", None, None),
        MaintenanceTarget::Subsystem(id) => ("subsystem", Some(id.0), None),
        MaintenanceTarget::Route(id) => ("route", None, Some(id.0)),
    }
}

/// A row's columns before the stored text is re-validated.
struct RawRow {
    id: i64,
    scope: String,
    subsystem_id: Option<i64>,
    route_id: Option<i64>,
    message: String,
    body: String,
    content_type: String,
    retry_after_secs: Option<u32>,
    bypass_cidrs: String,
    bypass_header: String,
    bypass_secret_hash: Option<Vec<u8>>,
    expires_at_ms: Option<i64>,
    set_by: String,
    set_at_ms: i64,
}

impl RawRow {
    fn into_row(self) -> Result<MaintenanceRow, StoreError> {
        let id = self.id;
        let corrupt = |why: String| StoreError::Internal(format!("maintenance row {id}: {why}"));
        let target = match (self.scope.as_str(), self.subsystem_id, self.route_id) {
            ("global", None, None) => MaintenanceTarget::Global,
            ("subsystem", Some(subsystem), None) => {
                MaintenanceTarget::Subsystem(SubsystemId(subsystem))
            }
            ("route", None, Some(route)) => MaintenanceTarget::Route(RouteId(route)),
            _ => return Err(corrupt("scope and target ids do not match".into())),
        };
        let body = if self.body.is_empty() {
            None
        } else {
            Some(CustomBody {
                content_type: BodyContentType::parse(&self.content_type)
                    .map_err(|err| corrupt(err.to_string()))?,
                text: self.body,
            })
        };
        let cidr_texts: Vec<String> = serde_json::from_str(&self.bypass_cidrs)
            .map_err(|err| corrupt(format!("unreadable bypass networks: {err}")))?;
        let cidrs = cidr_texts
            .iter()
            .map(|text| Cidr::parse("bypass.cidrs", text))
            .collect::<Result<_, _>>()
            .map_err(|err| corrupt(err.to_string()))?;
        let header = if self.bypass_header.is_empty() {
            None
        } else {
            let hash = self
                .bypass_secret_hash
                .as_deref()
                .and_then(SecretHash::from_slice)
                .ok_or_else(|| corrupt("bypass header without a valid secret hash".into()))?;
            Some(BypassHeader {
                name: self.bypass_header,
                secret_hash: hash,
            })
        };
        let settings = MaintenanceSettings {
            message: self.message,
            body,
            retry_after_secs: self.retry_after_secs,
            bypass: BypassRules { cidrs, header },
            expires_at_ms: self.expires_at_ms,
            set_by: self.set_by,
            set_at_ms: self.set_at_ms,
        };
        settings
            .validate()
            .map_err(|err| corrupt(err.to_string()))?;
        Ok(MaintenanceRow {
            id,
            target,
            settings,
        })
    }
}
