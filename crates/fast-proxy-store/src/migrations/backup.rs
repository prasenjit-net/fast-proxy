//! The copy of a database taken before its schema is upgraded, so an operator can
//! go back to the previous release (stop it, put the copy in place, start the old
//! binary). The copy holds what the database holds (certificate ciphertext, token
//! hashes, samples), so it is created readable by its owner only.

use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::clock::now_ms;
use crate::database::create_private_file_new;
use crate::StoreError;

/// How many pre-upgrade copies of one database are kept; older ones are removed.
const KEPT_COPIES: usize = 3;
/// A copy younger than this is never removed (see [`remove_old_copies`]).
const SETTLE_MS: i64 = 60 * 60 * 1000;

/// Writes a consistent copy of the database at `db` to
/// `<db>.pre-v<to>-<unix ms>.bak` and removes older copies beyond the newest
/// [`KEPT_COPIES`].
pub(super) fn copy_before_upgrade(
    conn: &Connection,
    db: &Path,
    to: i64,
) -> Result<PathBuf, StoreError> {
    let copy = claim_copy_path(db, to)?;
    let target = copy
        .to_str()
        .ok_or_else(|| StoreError::Internal("the database path is not valid UTF-8".into()))?;
    conn.execute("VACUUM INTO ?1", [target])?;
    remove_old_copies(db, now_ms())?;
    Ok(copy)
}

/// Creates the empty copy file (VACUUM INTO accepts an existing empty file, so
/// the copy keeps the mode set here). Two processes upgrading at the same
/// millisecond must not share a file, so a taken name moves on by a millisecond.
fn claim_copy_path(db: &Path, to: i64) -> Result<PathBuf, StoreError> {
    let mut at_ms = now_ms();
    loop {
        let copy = copy_path(db, to, at_ms);
        match create_private_file_new(&copy) {
            Ok(()) => return Ok(copy),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => at_ms += 1,
            Err(err) => return Err(err.into()),
        }
    }
}

fn copy_path(db: &Path, to: i64, at_ms: i64) -> PathBuf {
    let mut name = db.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".pre-v{to}-{at_ms}.bak"));
    db.with_file_name(name)
}

/// The time in a copy's name, if `name` is a pre-upgrade copy of the file `db_name`.
fn copy_time(name: &str, db_name: &str) -> Option<i64> {
    let rest = name.strip_prefix(db_name)?.strip_prefix(".pre-v")?;
    let (_, at) = rest.strip_suffix(".bak")?.split_once('-')?;
    at.parse().ok()
}

/// Removes copies beyond the newest [`KEPT_COPIES`], but none taken in the last
/// [`SETTLE_MS`]: several processes upgrading one file at once each take a copy,
/// and one must not remove a copy another is still writing.
fn remove_old_copies(db: &Path, now_ms: i64) -> std::io::Result<()> {
    let Some(db_name) = db.file_name().and_then(|name| name.to_str()) else {
        return Ok(());
    };
    let dir = match db.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        Some(dir) => dir.to_path_buf(),
        None => PathBuf::from("."),
    };
    let mut copies: Vec<(i64, PathBuf)> = std::fs::read_dir(&dir)?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let at = copy_time(entry.file_name().to_str()?, db_name)?;
            Some((at, entry.path()))
        })
        .collect();
    copies.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    let removable = copies
        .into_iter()
        .skip(KEPT_COPIES)
        .filter(|(at, _)| now_ms.saturating_sub(*at) > SETTLE_MS);
    for (_, old) in removable {
        match std::fs::remove_file(old) {
            // Another process removed it first.
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            other => other?,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_copy_is_named_after_the_database_the_version_and_the_time() {
        let copy = copy_path(Path::new("/var/lib/fp/fast-proxy.db"), 6, 1_700_000_000_000);
        assert_eq!(
            copy,
            Path::new("/var/lib/fp/fast-proxy.db.pre-v6-1700000000000.bak")
        );
        let name = copy.file_name().unwrap().to_str().unwrap();
        assert_eq!(copy_time(name, "fast-proxy.db"), Some(1_700_000_000_000));
    }

    #[test]
    fn only_copies_of_this_database_are_recognised() {
        for other in [
            "fast-proxy.db",
            "fast-proxy.db-wal",
            "other.db.pre-v6-1.bak",
            "fast-proxy.db.pre-v6-x.bak",
            "fast-proxy.db.pre-v6-1.bak.tmp",
        ] {
            assert_eq!(copy_time(other, "fast-proxy.db"), None, "{other}");
        }
    }

    #[test]
    fn the_newest_copies_are_kept_and_nothing_else_is_touched() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("fp.db");
        for at in [10, 40, 20, 50, 30] {
            std::fs::write(copy_path(&db, 6, at), b"").unwrap();
        }
        std::fs::write(dir.path().join("fp.db-wal"), b"").unwrap();
        std::fs::write(dir.path().join("unrelated.bak"), b"").unwrap();
        remove_old_copies(&db, 50 + SETTLE_MS + 1).unwrap();
        assert_eq!(
            files_in(dir.path()),
            [
                "fp.db-wal",
                "fp.db.pre-v6-30.bak",
                "fp.db.pre-v6-40.bak",
                "fp.db.pre-v6-50.bak",
                "unrelated.bak"
            ]
        );
    }

    #[test]
    fn a_copy_taken_within_the_settle_time_is_never_removed() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("fp.db");
        for at in [10, 20, 30, 40, 50] {
            std::fs::write(copy_path(&db, 6, at), b"").unwrap();
        }
        // 10 is old enough to go; 20 is not, so it stays although it is fifth newest.
        remove_old_copies(&db, 10 + SETTLE_MS + 1).unwrap();
        assert_eq!(
            files_in(dir.path()),
            [
                "fp.db.pre-v6-20.bak",
                "fp.db.pre-v6-30.bak",
                "fp.db.pre-v6-40.bak",
                "fp.db.pre-v6-50.bak"
            ]
        );
    }

    fn files_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }
}
