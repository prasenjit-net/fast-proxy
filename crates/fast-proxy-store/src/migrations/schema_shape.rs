//! The shape of a database schema, for comparing a new database with an upgraded
//! one. SQLite keeps `ALTER TABLE ... ADD COLUMN` by splicing text into the stored
//! `CREATE TABLE`, so the text of the two differs even when the tables are the
//! same; the shape is what the engine reports about each table instead.

use rusqlite::types::Value;
use rusqlite::Connection;

/// Everything that makes two schemas behave the same.
#[derive(Debug, PartialEq)]
pub(super) struct SchemaShape {
    pub tables: Vec<TableShape>,
    /// `CREATE INDEX` statements (named indexes; implicit ones are in each table).
    pub indexes: Vec<String>,
    /// Views and triggers, by their SQL.
    pub others: Vec<String>,
}

/// One table: its columns in order, keys, implicit indexes and `CHECK` clauses.
#[derive(Debug, PartialEq)]
pub(super) struct TableShape {
    pub name: String,
    pub columns: Vec<Vec<Value>>,
    pub foreign_keys: Vec<Vec<Value>>,
    pub implicit_indexes: Vec<Vec<Value>>,
    pub checks: Vec<String>,
}

pub(super) fn shape_of(conn: &Connection) -> SchemaShape {
    let tables = names(conn, "table")
        .into_iter()
        .map(|(name, sql)| TableShape {
            columns: rows(conn, "SELECT * FROM pragma_table_xinfo(?1)", &name),
            foreign_keys: rows(conn, "SELECT * FROM pragma_foreign_key_list(?1)", &name),
            implicit_indexes: implicit_indexes(conn, &name),
            checks: checks(&sql),
            name,
        })
        .collect();
    SchemaShape {
        tables,
        indexes: sql_of(conn, "index"),
        others: [sql_of(conn, "view"), sql_of(conn, "trigger")].concat(),
    }
}

fn names(conn: &Connection, kind: &str) -> Vec<(String, String)> {
    let mut statement = conn
        .prepare(
            "SELECT name, sql FROM sqlite_schema \
             WHERE type = ?1 AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .unwrap();
    statement
        .query_map([kind], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn sql_of(conn: &Connection, kind: &str) -> Vec<String> {
    let mut statement = conn
        .prepare(
            "SELECT sql FROM sqlite_schema \
             WHERE type = ?1 AND sql IS NOT NULL ORDER BY name",
        )
        .unwrap();
    statement
        .query_map([kind], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn rows(conn: &Connection, query: &str, table: &str) -> Vec<Vec<Value>> {
    let mut statement = conn.prepare(query).unwrap();
    let width = statement.column_count();
    statement
        .query_map([table], |row| (0..width).map(|i| row.get(i)).collect())
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// The indexes SQLite makes for `UNIQUE` and `PRIMARY KEY` clauses, with their columns.
fn implicit_indexes(conn: &Connection, table: &str) -> Vec<Vec<Value>> {
    let mut statement = conn
        .prepare(
            "SELECT l.\"unique\", l.origin, l.partial, i.name \
             FROM pragma_index_list(?1) AS l, pragma_index_info(l.name) AS i \
             WHERE l.origin != 'c' ORDER BY l.name, i.seqno",
        )
        .unwrap();
    statement
        .query_map([table], |row| (0..4).map(|i| row.get(i)).collect())
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// The `CHECK (...)` clauses of a `CREATE TABLE`, with whitespace collapsed.
fn checks(sql: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = sql;
    while let Some(at) = rest.find("CHECK") {
        let clause = &rest[at..];
        let Some(open) = clause.find('(') else { break };
        let mut depth = 0;
        let mut end = clause.len();
        for (i, c) in clause[open..].char_indices() {
            depth += match c {
                '(' => 1,
                ')' => -1,
                _ => 0,
            };
            if depth == 0 {
                end = open + i + 1;
                break;
            }
        }
        found.push(
            clause[..end]
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
        );
        rest = &clause[end..];
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_are_found_with_nested_parentheses() {
        let sql =
            "CREATE TABLE t (id INTEGER CHECK (id = 1), n INTEGER CHECK ((n > 0)  AND (n < 9)))";
        assert_eq!(
            checks(sql),
            ["CHECK (id = 1)", "CHECK ((n > 0) AND (n < 9))"]
        );
    }

    #[test]
    fn a_different_column_default_changes_the_shape() {
        let a = Connection::open_in_memory().unwrap();
        a.execute_batch("CREATE TABLE t (n INTEGER NOT NULL DEFAULT 0)")
            .unwrap();
        let b = Connection::open_in_memory().unwrap();
        b.execute_batch("CREATE TABLE t (n INTEGER NOT NULL DEFAULT 1)")
            .unwrap();
        assert_ne!(shape_of(&a), shape_of(&b));
    }

    #[test]
    fn an_added_column_has_the_shape_of_a_declared_one() {
        let declared = Connection::open_in_memory().unwrap();
        declared
            .execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY, n INTEGER NOT NULL DEFAULT 0)")
            .unwrap();
        let added = Connection::open_in_memory().unwrap();
        added
            .execute_batch(
                "CREATE TABLE t (id INTEGER PRIMARY KEY);
                 ALTER TABLE t ADD COLUMN n INTEGER NOT NULL DEFAULT 0;",
            )
            .unwrap();
        assert_eq!(shape_of(&declared), shape_of(&added));
    }
}
