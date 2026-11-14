//! Sampling settings: one global row and at most one override per subsystem or
//! route. Not part of `Config`; a rollback keeps them (see
//! [`reinsert_surviving`]). Overrides are removed with their subsystem or route.

use fast_proxy_model::{
    RouteId, SamplingOverride, SamplingSettings, SamplingTarget, SubsystemId, TriggerSet,
};
use rusqlite::{params, Connection, OptionalExtension, Row};

use super::ensure_subsystem;
use crate::StoreError;

/// A stored override.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SamplingOverrideRow {
    pub id: i64,
    pub target: SamplingTarget,
    pub settings: SamplingOverride,
}

/// The global settings: what was last written, or the defaults when nothing
/// has been written yet. A stored row that no longer validates is corruption
/// and fails the call (`Internal`).
pub fn get_settings(conn: &Connection) -> Result<SamplingSettings, StoreError> {
    let stored = conn
        .query_row(
            "SELECT enabled, triggers, slow_threshold_ms, slow_p99_factor_x10, slow_p99_floor_ms,
                    client_errors_per_min, oversized_request, oversized_response,
                    baseline_per_mille, rate_per_sec, rate_burst, storm_burst,
                    storm_interval_secs, retention_days, max_rows, max_bytes,
                    body_request_cap, body_response_cap, extra_headers, redact_headers,
                    redact_query_params, redact_body_fields, updated_by, updated_at
             FROM sampling_settings WHERE id = 1",
            [],
            read_settings,
        )
        .optional()?;
    let Some(settings) = stored else {
        return Ok(SamplingSettings::default());
    };
    let settings = settings?;
    settings
        .validate()
        .map_err(|err| StoreError::Internal(format!("stored sampling settings: {err}")))?;
    Ok(settings)
}

/// Replaces the global settings. Invalid settings are refused with the field.
pub fn set_settings(conn: &Connection, settings: &SamplingSettings) -> Result<(), StoreError> {
    settings.validate()?;
    conn.execute(
        "INSERT OR REPLACE INTO sampling_settings
           (id, enabled, triggers, slow_threshold_ms, slow_p99_factor_x10, slow_p99_floor_ms,
            client_errors_per_min, oversized_request, oversized_response, baseline_per_mille,
            rate_per_sec, rate_burst, storm_burst, storm_interval_secs, retention_days,
            max_rows, max_bytes, body_request_cap, body_response_cap, extra_headers,
            redact_headers, redact_query_params, redact_body_fields, updated_by, updated_at)
         VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17,
                 ?18, ?19, ?20, ?21, ?22, ?23, ?24)",
        params![
            settings.enabled,
            settings.triggers.bits(),
            settings.slow_threshold_ms,
            settings.slow_p99_factor_x10,
            settings.slow_p99_floor_ms,
            settings.client_errors_per_min,
            as_stored(settings.oversized_request_bytes)?,
            as_stored(settings.oversized_response_bytes)?,
            settings.baseline_per_mille,
            settings.rate_per_sec,
            settings.rate_burst,
            settings.storm_burst,
            settings.storm_interval_secs,
            settings.retention_days,
            as_stored(settings.max_rows)?,
            as_stored(settings.max_bytes)?,
            settings.body_request_cap,
            settings.body_response_cap,
            names_json(&settings.extra_headers)?,
            names_json(&settings.redact_headers)?,
            names_json(&settings.redact_query_params)?,
            names_json(&settings.redact_body_fields)?,
            settings.updated_by,
            settings.updated_at_ms,
        ],
    )?;
    Ok(())
}

/// Sets the override of `target`, replacing any earlier one. Invalid settings
/// are refused with the field; a missing target is `NotFound`.
pub fn set_override(
    conn: &Connection,
    target: SamplingTarget,
    settings: &SamplingOverride,
) -> Result<(), StoreError> {
    settings.validate()?;
    ensure_target_exists(conn, target)?;
    let (scope, subsystem_id, route_id) = target_columns(target);
    clear_override(conn, target)?;
    conn.execute(
        "INSERT INTO sampling_override
           (scope, subsystem_id, route_id, enabled, capture_bodies, baseline_per_mille,
            slow_threshold_ms, slow_p99_factor_x10, set_by, set_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            scope,
            subsystem_id,
            route_id,
            settings.enabled,
            settings.capture_bodies,
            settings.baseline_per_mille,
            settings.slow_threshold_ms,
            settings.slow_p99_factor_x10,
            settings.set_by,
            settings.set_at_ms,
        ],
    )?;
    Ok(())
}

/// Removes the override of `target`; returns whether there was one, so
/// clearing a cleared override is a harmless no-op.
pub fn clear_override(conn: &Connection, target: SamplingTarget) -> Result<bool, StoreError> {
    let (scope, subsystem_id, route_id) = target_columns(target);
    let changed = conn.execute(
        "DELETE FROM sampling_override
         WHERE scope = ?1 AND COALESCE(subsystem_id, 0) = ?2 AND COALESCE(route_id, 0) = ?3",
        params![scope, subsystem_id.unwrap_or(0), route_id.unwrap_or(0)],
    )?;
    Ok(changed > 0)
}

/// Every stored override, oldest first.
pub fn list_overrides(conn: &Connection) -> Result<Vec<SamplingOverrideRow>, StoreError> {
    let mut statement = conn.prepare(
        "SELECT id, scope, subsystem_id, route_id, enabled, capture_bodies, baseline_per_mille,
                slow_threshold_ms, slow_p99_factor_x10, set_by, set_at
         FROM sampling_override ORDER BY id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok(RawOverride {
            id: row.get(0)?,
            scope: row.get(1)?,
            subsystem_id: row.get(2)?,
            route_id: row.get(3)?,
            settings: SamplingOverride {
                enabled: row.get(4)?,
                capture_bodies: row.get(5)?,
                baseline_per_mille: row.get(6)?,
                slow_threshold_ms: row.get(7)?,
                slow_p99_factor_x10: row.get(8)?,
                set_by: row.get(9)?,
                set_at_ms: row.get(10)?,
            },
        })
    })?;
    rows.map(|row| row?.into_row()).collect()
}

/// Puts back overrides taken from [`list_overrides`] before a rollback replaced
/// the configuration. Ids are preserved by a rollback, so an override survives
/// when its subsystem or route still exists and goes with its target otherwise.
pub fn reinsert_surviving(
    conn: &Connection,
    rows: &[SamplingOverrideRow],
) -> Result<(), StoreError> {
    for row in rows {
        match set_override(conn, row.target, &row.settings) {
            Ok(()) | Err(StoreError::NotFound(_)) => {}
            Err(other) => return Err(other),
        }
    }
    Ok(())
}

fn ensure_target_exists(conn: &Connection, target: SamplingTarget) -> Result<(), StoreError> {
    match target {
        SamplingTarget::Subsystem(id) => ensure_subsystem(conn, id),
        SamplingTarget::Route(id) => {
            let found = conn
                .query_row("SELECT 1 FROM route WHERE id = ?1", [id.0], |_| Ok(()))
                .optional()?;
            found.ok_or_else(|| StoreError::NotFound(format!("route {}", id.0)))
        }
    }
}

fn target_columns(target: SamplingTarget) -> (&'static str, Option<i64>, Option<i64>) {
    match target {
        SamplingTarget::Subsystem(id) => ("subsystem", Some(id.0), None),
        SamplingTarget::Route(id) => ("route", None, Some(id.0)),
    }
}

fn as_stored(value: u64) -> Result<i64, StoreError> {
    i64::try_from(value)
        .map_err(|_| StoreError::Internal("a sampling limit is out of range".into()))
}

fn names_json(names: &[String]) -> Result<String, StoreError> {
    serde_json::to_string(names).map_err(|err| StoreError::Internal(err.to_string()))
}

fn names_from_json(text: &str, column: &str) -> Result<Vec<String>, StoreError> {
    serde_json::from_str(text)
        .map_err(|err| StoreError::Internal(format!("stored sampling {column}: {err}")))
}

/// The settings row's columns; a column that cannot be rebuilt (an unknown
/// trigger bit, unreadable JSON) is corruption.
fn read_settings(row: &Row<'_>) -> rusqlite::Result<Result<SamplingSettings, StoreError>> {
    let bits: u32 = row.get(1)?;
    let lists = [
        row.get::<_, String>(18)?,
        row.get::<_, String>(19)?,
        row.get::<_, String>(20)?,
        row.get::<_, String>(21)?,
    ];
    let numbers = (
        row.get::<_, i64>(6)?,
        row.get::<_, i64>(7)?,
        row.get::<_, i64>(14)?,
        row.get::<_, i64>(15)?,
    );
    let settings = SamplingSettings {
        enabled: row.get(0)?,
        triggers: TriggerSet::from_bits(bits).unwrap_or_default(),
        slow_threshold_ms: row.get(2)?,
        slow_p99_factor_x10: row.get(3)?,
        slow_p99_floor_ms: row.get(4)?,
        client_errors_per_min: row.get(5)?,
        oversized_request_bytes: u64::try_from(numbers.0).unwrap_or(0),
        oversized_response_bytes: u64::try_from(numbers.1).unwrap_or(0),
        baseline_per_mille: row.get(8)?,
        rate_per_sec: row.get(9)?,
        rate_burst: row.get(10)?,
        storm_burst: row.get(11)?,
        storm_interval_secs: row.get(12)?,
        retention_days: row.get(13)?,
        max_rows: u64::try_from(numbers.2).unwrap_or(0),
        max_bytes: u64::try_from(numbers.3).unwrap_or(0),
        body_request_cap: row.get(16)?,
        body_response_cap: row.get(17)?,
        extra_headers: Vec::new(),
        redact_headers: Vec::new(),
        redact_query_params: Vec::new(),
        redact_body_fields: Vec::new(),
        updated_by: row.get(22)?,
        updated_at_ms: row.get(23)?,
    };
    Ok(finish_settings(settings, bits, &lists))
}

fn finish_settings(
    mut settings: SamplingSettings,
    bits: u32,
    lists: &[String; 4],
) -> Result<SamplingSettings, StoreError> {
    if TriggerSet::from_bits(bits).is_none() {
        return Err(StoreError::Internal(
            "stored sampling settings name an unknown trigger".into(),
        ));
    }
    settings.extra_headers = names_from_json(&lists[0], "extra_headers")?;
    settings.redact_headers = names_from_json(&lists[1], "redact_headers")?;
    settings.redact_query_params = names_from_json(&lists[2], "redact_query_params")?;
    settings.redact_body_fields = names_from_json(&lists[3], "redact_body_fields")?;
    Ok(settings)
}

struct RawOverride {
    id: i64,
    scope: String,
    subsystem_id: Option<i64>,
    route_id: Option<i64>,
    settings: SamplingOverride,
}

impl RawOverride {
    fn into_row(self) -> Result<SamplingOverrideRow, StoreError> {
        let id = self.id;
        let corrupt = |why: String| StoreError::Internal(format!("sampling override {id}: {why}"));
        let target = match (self.scope.as_str(), self.subsystem_id, self.route_id) {
            ("subsystem", Some(subsystem), None) => {
                SamplingTarget::Subsystem(SubsystemId(subsystem))
            }
            ("route", None, Some(route)) => SamplingTarget::Route(RouteId(route)),
            _ => return Err(corrupt("scope and target ids do not match".into())),
        };
        self.settings
            .validate()
            .map_err(|err| corrupt(err.to_string()))?;
        Ok(SamplingOverrideRow {
            id,
            target,
            settings: self.settings,
        })
    }
}
