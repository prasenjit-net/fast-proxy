//! Rate-limit policies of subsystems and routes (ADR 0030). They are part of
//! `Config`: written inside a configuration change, held by every revision and
//! put back by a rollback. A subsystem has at most one policy of its own, and a
//! route at most one, which replaces the subsystem's for that route.

use std::collections::HashMap;
use std::time::Duration;

use fast_proxy_model::{RateLimitHeader, RateLimitKey, RateLimitPolicy, RouteId, SubsystemId};
use rusqlite::{params, Connection, OptionalExtension, Row};

use super::{ensure_subsystem, touch};
use crate::StoreError;

const CLIENT: &str = "client";
const HEADER: &str = "header";

/// Sets the policy of a subsystem (`route` is `None`) or of one of its routes,
/// replacing the one there was.
pub fn set(
    conn: &Connection,
    subsystem: SubsystemId,
    route: Option<RouteId>,
    policy: &RateLimitPolicy,
) -> Result<(), StoreError> {
    policy.validate()?;
    ensure_target(conn, subsystem, route)?;
    delete(conn, subsystem, route)?;
    insert(conn, subsystem, route, policy)?;
    touch(conn, subsystem)
}

/// Removes the policy of a subsystem or route; `false` when it had none.
pub fn clear(
    conn: &Connection,
    subsystem: SubsystemId,
    route: Option<RouteId>,
) -> Result<bool, StoreError> {
    ensure_target(conn, subsystem, route)?;
    let removed = delete(conn, subsystem, route)? > 0;
    if removed {
        touch(conn, subsystem)?;
    }
    Ok(removed)
}

/// A subsystem's own policy and those of its routes, by route id.
pub(crate) type SubsystemPolicies = (Option<RateLimitPolicy>, HashMap<i64, RateLimitPolicy>);

/// Every policy of one subsystem, for loading the configuration.
pub(crate) fn load(
    conn: &Connection,
    subsystem: SubsystemId,
) -> Result<SubsystemPolicies, StoreError> {
    let mut statement = conn.prepare(
        "SELECT route_id, rate, period_secs, burst, key_kind, key_header, ipv6_prefix
         FROM rate_limit_policy WHERE subsystem_id = ?1",
    )?;
    let rows = statement.query_map([subsystem.0], |row| Ok((row.get(0)?, read(row)?)))?;
    let mut own = None;
    let mut routes = HashMap::new();
    for row in rows {
        let (route, policy): (Option<i64>, Result<RateLimitPolicy, StoreError>) = row?;
        match route {
            None => own = Some(policy?),
            Some(route) => {
                routes.insert(route, policy?);
            }
        }
    }
    Ok((own, routes))
}

/// Writes a policy without checks or touching the subsystem: for a rollback,
/// which rebuilds every row from a revision.
pub(crate) fn insert(
    conn: &Connection,
    subsystem: SubsystemId,
    route: Option<RouteId>,
    policy: &RateLimitPolicy,
) -> Result<(), StoreError> {
    let (kind, header) = match &policy.key {
        RateLimitKey::Client => (CLIENT, None),
        RateLimitKey::Header(name) => (HEADER, Some(name.as_str())),
    };
    conn.execute(
        "INSERT INTO rate_limit_policy
           (subsystem_id, route_id, rate, period_secs, burst, key_kind, key_header, ipv6_prefix)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            subsystem.0,
            route.map(|route| route.0),
            policy.rate,
            i64::try_from(policy.period.as_secs()).unwrap_or(i64::MAX),
            policy.burst,
            kind,
            header,
            policy.ipv6_prefix,
        ],
    )?;
    Ok(())
}

fn delete(
    conn: &Connection,
    subsystem: SubsystemId,
    route: Option<RouteId>,
) -> Result<usize, StoreError> {
    let removed = match route {
        None => conn.execute(
            "DELETE FROM rate_limit_policy WHERE subsystem_id = ?1 AND route_id IS NULL",
            [subsystem.0],
        )?,
        Some(route) => conn.execute(
            "DELETE FROM rate_limit_policy WHERE subsystem_id = ?1 AND route_id = ?2",
            params![subsystem.0, route.0],
        )?,
    };
    Ok(removed)
}

/// Fails with `NotFound` unless the subsystem exists and, when given, the route
/// belongs to it.
fn ensure_target(
    conn: &Connection,
    subsystem: SubsystemId,
    route: Option<RouteId>,
) -> Result<(), StoreError> {
    ensure_subsystem(conn, subsystem)?;
    let Some(route) = route else {
        return Ok(());
    };
    let found = conn
        .query_row(
            "SELECT 1 FROM route WHERE id = ?1 AND subsystem_id = ?2",
            params![route.0, subsystem.0],
            |_| Ok(()),
        )
        .optional()?;
    found.ok_or_else(|| StoreError::NotFound(format!("route {}", route.0)))
}

/// A stored policy, validated again on the way out like every stored value.
fn read(row: &Row<'_>) -> rusqlite::Result<Result<RateLimitPolicy, StoreError>> {
    let kind: String = row.get(4)?;
    let header: Option<String> = row.get(5)?;
    let period_secs = u64::try_from(row.get::<_, i64>(2)?).unwrap_or(0);
    let key = match (kind.as_str(), header) {
        (CLIENT, _) => Ok(RateLimitKey::Client),
        (HEADER, Some(name)) => RateLimitHeader::parse("rateLimit.header", &name)
            .map(RateLimitKey::Header)
            .map_err(|err| corrupt(&err.message)),
        _ => Err(corrupt("unknown key")),
    };
    let policy = key.and_then(|key| {
        let policy = RateLimitPolicy {
            rate: row.get(1).map_err(StoreError::from)?,
            period: Duration::from_secs(period_secs),
            burst: row.get(3).map_err(StoreError::from)?,
            key,
            ipv6_prefix: row.get(6).map_err(StoreError::from)?,
        };
        policy.validate().map_err(|err| corrupt(&err.message))?;
        Ok(policy)
    });
    Ok(policy)
}

fn corrupt(detail: &str) -> StoreError {
    StoreError::Internal(format!("stored rate-limit policy is invalid: {detail}"))
}

#[cfg(test)]
mod tests {
    use fast_proxy_model::DEFAULT_IPV6_PREFIX;

    use super::*;
    use crate::repository::routes;
    use crate::test_support::{commit, route_fields, seed};
    use crate::Database;

    fn policy(rate: u32) -> RateLimitPolicy {
        RateLimitPolicy {
            rate,
            period: Duration::from_secs(1),
            burst: 5,
            key: RateLimitKey::Client,
            ipv6_prefix: DEFAULT_IPV6_PREFIX,
        }
    }

    fn header_policy() -> RateLimitPolicy {
        RateLimitPolicy {
            key: RateLimitKey::Header(RateLimitHeader::parse("h", "X-Api-Key").unwrap()),
            ipv6_prefix: 56,
            ..policy(50)
        }
    }

    #[test]
    fn a_subsystem_and_its_routes_each_hold_one_policy() {
        let mut db = Database::open_in_memory().unwrap();
        let shop = seed(&mut db, "shop", "shop.example.com");
        let api = commit(&mut db, "route", |c| {
            routes::create(c, shop.subsystem, &route_fields("/api", shop.group))
        })
        .unwrap()
        .value;
        commit(&mut db, "limits", |c| {
            set(c, shop.subsystem, None, &policy(10))?;
            set(c, shop.subsystem, None, &policy(20))?;
            set(c, shop.subsystem, Some(api), &header_policy())
        })
        .unwrap();
        let config = db.load_all().unwrap();
        let shop_config = &config.subsystems[0];
        assert_eq!(
            shop_config.rate_limit,
            Some(policy(20)),
            "replaced, not added"
        );
        let api_route = shop_config.routes.iter().find(|r| r.id == api).unwrap();
        assert_eq!(api_route.rate_limit, Some(header_policy()));
        let root = shop_config.routes.iter().find(|r| r.id != api).unwrap();
        assert_eq!(root.rate_limit, None, "a route without its own has none");
    }

    #[test]
    fn clearing_reports_whether_there_was_a_policy() {
        let mut db = Database::open_in_memory().unwrap();
        let shop = seed(&mut db, "shop", "shop.example.com");
        let cleared = commit(&mut db, "limits", |c| {
            set(c, shop.subsystem, None, &policy(10))?;
            Ok((
                clear(c, shop.subsystem, None)?,
                clear(c, shop.subsystem, None)?,
            ))
        })
        .unwrap()
        .value;
        assert_eq!(cleared, (true, false));
        assert_eq!(db.load_all().unwrap().subsystems[0].rate_limit, None);
    }

    #[test]
    fn a_route_of_another_subsystem_or_a_bad_policy_is_refused() {
        let mut db = Database::open_in_memory().unwrap();
        let shop = seed(&mut db, "shop", "shop.example.com");
        let blog = seed(&mut db, "blog", "blog.example.com");
        let blog_route = db.load_all().unwrap().subsystems[1].routes[0].id;
        let err = commit(&mut db, "x", |c| {
            set(c, shop.subsystem, Some(blog_route), &policy(10))
        })
        .unwrap_err();
        assert!(matches!(err, StoreError::NotFound(_)), "{err}");
        let err = commit(&mut db, "x", |c| set(c, blog.subsystem, None, &policy(0))).unwrap_err();
        assert!(matches!(err, StoreError::Invalid(_)), "{err}");
    }

    #[test]
    fn deleting_a_route_or_subsystem_removes_its_policies() {
        let mut db = Database::open_in_memory().unwrap();
        let shop = seed(&mut db, "shop", "shop.example.com");
        let route = db.load_all().unwrap().subsystems[0].routes[0].id;
        commit(&mut db, "limits", |c| {
            set(c, shop.subsystem, None, &policy(10))?;
            set(c, shop.subsystem, Some(route), &policy(20))
        })
        .unwrap();
        commit(&mut db, "route removed", |c| {
            routes::delete(c, shop.subsystem, route)
        })
        .unwrap();
        let count = |db: &Database| -> i64 {
            db.read(
                |c| Ok(c.query_row("SELECT count(*) FROM rate_limit_policy", [], |r| r.get(0))?),
            )
            .unwrap()
        };
        assert_eq!(count(&db), 1);
        commit(&mut db, "gone", |c| {
            crate::repository::subsystems::delete(c, shop.subsystem)
        })
        .unwrap();
        assert_eq!(count(&db), 0);
    }

    #[test]
    fn a_tampered_row_is_reported_not_trusted() {
        let mut db = Database::open_in_memory().unwrap();
        let shop = seed(&mut db, "shop", "shop.example.com");
        commit(&mut db, "limit", |c| {
            set(c, shop.subsystem, None, &policy(10))
        })
        .unwrap();
        db.read(|c| Ok(c.execute("UPDATE rate_limit_policy SET rate = 0", [])?))
            .unwrap();
        assert!(matches!(db.load_all(), Err(StoreError::Internal(_))));
    }
}
