use anyhow::{Context, Result};
use rusqlite::Connection;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::app::ObjectInfo;

/// Directory (under `$HOME/.cache`) where the SQLite database lives.
pub const CACHE_DIR_NAME: &str = "s3-tui";
/// Name of the SQLite database file holding the object listings.
pub const CACHE_DB_NAME: &str = "cache.db";

/// Absolute path of the cache database, honouring `$HOME` (or the system
/// temp dir when `$HOME` is unset).
pub fn cache_db_path() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(".cache")
        .join(CACHE_DIR_NAME)
        .join(CACHE_DB_NAME)
}

/// A writable handle over the SQLite object cache.
///
/// The schema keeps one row per `(bucket, prefix, key)` so listings from
/// different buckets and folders never collide. Rows are upserted on write and
/// pruned when a whole folder is re-listed.
pub struct ObjectCache {
    conn: Connection,
}

fn row_to_object(row: &rusqlite::Row<'_>) -> rusqlite::Result<ObjectInfo> {
    let is_folder: bool = row.get(3)?;
    let storage_class: String = row.get(4)?;
    let storage_class = if is_folder {
        crate::app::StorageClass::Folder
    } else {
        crate::app::StorageClass::Unknown(storage_class)
    };
    Ok(ObjectInfo {
        key: row.get(0)?,
        size: row.get::<_, Option<i64>>(1)?.map(|v| v as u64),
        last_modified: row
            .get::<_, Option<i64>>(2)?
            .and_then(|ts| chrono::DateTime::from_timestamp(ts, 0)),
        is_folder,
        storage_class,
    })
}

impl ObjectCache {
    /// Open (creating if needed) the cache database at its default location.
    pub fn open_default() -> Result<Self> {
        let path = cache_db_path();
        Self::open(&path)
    }

    /// Open (creating if needed) the cache database at `path`.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create cache dir {}", parent.display()))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("failed to open cache db {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .context("failed to set WAL journal mode")?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS objects (
                bucket TEXT NOT NULL,
                prefix TEXT NOT NULL,
                key TEXT NOT NULL,
                size INTEGER,
                last_modified INTEGER,
                is_folder INTEGER NOT NULL DEFAULT 0,
                storage_class TEXT NOT NULL DEFAULT '',
                PRIMARY KEY (bucket, prefix, key)
            );
            CREATE TABLE IF NOT EXISTS listings (
                bucket TEXT NOT NULL,
                prefix TEXT NOT NULL,
                PRIMARY KEY (bucket, prefix)
            );",
        )
        .context("failed to create tables")?;
        Ok(Self { conn })
    }

    fn ensure_prefix_rows(&self, bucket: &str, prefix: &str, objects: &[ObjectInfo]) -> Result<()> {
        // Every key in the rows is already prefixed with `prefix`.
        let wanted: HashSet<&str> = objects.iter().map(|o| o.key.as_str()).collect();

        let existing: Vec<String> = {
            let mut stmt = self
                .conn
                .prepare("SELECT key FROM objects WHERE bucket = ?1 AND prefix = ?2")
                .context("failed to prepare select query")?;
            let keys = stmt
                .query_map([bucket, prefix], |row| row.get(0))
                .with_context(|| format!("failed to query rows for s3://{bucket}/{prefix}"))?
                .collect::<Result<Vec<String>, _>>()
                .context("failed to iterate existing rows")?;
            stmt.finalize().ok();
            keys
        };

        let stale: Vec<&String> = existing
            .iter()
            .filter(|k| !wanted.contains(k.as_str()))
            .collect();
        if !stale.is_empty() {
            for key in &stale {
                self.conn
                    .execute(
                        "DELETE FROM objects WHERE bucket = ?1 AND prefix = ?2 AND key = ?3",
                        rusqlite::params![bucket, prefix, key],
                    )
                    .with_context(|| format!("failed to delete stale row {key}"))?;
            }
        }

        // Upsert every known object.
        let mut stmt = self
            .conn
            .prepare(
                "INSERT INTO objects (bucket, prefix, key, size, last_modified, is_folder, storage_class)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(bucket, prefix, key) DO UPDATE SET
                     size = excluded.size,
                     last_modified = excluded.last_modified,
                     is_folder = excluded.is_folder,
                     storage_class = excluded.storage_class",
            )
            .context("failed to prepare upsert statement")?;
        for obj in objects {
            let ts = obj.last_modified.map(|d| d.timestamp());
            let sc = match &obj.storage_class {
                crate::app::StorageClass::Folder => String::new(),
                other => other.to_string(),
            };
            stmt.execute(rusqlite::params![
                bucket,
                prefix,
                obj.key,
                obj.size.map(|s| s as i64),
                ts,
                obj.is_folder as i64,
                sc,
            ])
            .with_context(|| format!("failed to upsert row for {}", obj.key))?;
        }
        Ok(())
    }

    /// Replace the cached listing of `(bucket, prefix)` with `objects`,
    /// pruning stale rows and upserting current ones. The remote listing is
    /// the source of truth. Marks `(bucket, prefix)` as cacheable even when
    /// `objects` is empty.
    pub fn set_prefix(&self, bucket: &str, prefix: &str, objects: &[ObjectInfo]) -> Result<()> {
        self.ensure_prefix_rows(bucket, prefix, objects)?;
        self.conn
            .execute(
                "INSERT INTO listings (bucket, prefix) VALUES (?1, ?2)
                 ON CONFLICT(bucket, prefix) DO NOTHING",
                rusqlite::params![bucket, prefix],
            )
            .context("failed to mark listing as cached")?;
        Ok(())
    }

    /// `true` when the cache has a recorded listing for `(bucket, prefix)` (it
    /// may be empty). Used to decide whether a remote first-list is needed.
    pub fn listing_exists(&self, bucket: &str, prefix: &str) -> Result<bool> {
        let mut stmt = self
            .conn
            .prepare("SELECT 1 FROM listings WHERE bucket = ?1 AND prefix = ?2")
            .context("failed to prepare listing check")?;
        let exists = stmt
            .query_row(rusqlite::params![bucket, prefix], |_| Ok(()))
            .is_ok();
        Ok(exists)
    }

    /// Mark a single object as having been uploaded by this app. The row is
    /// kept even though the folder it belongs to may be pruned later by a full
    /// re-list; that is fine because uploads normally follow the last re-list
    /// and the next re-list will refresh from S3 anyway.
    pub fn upsert_object(
        &self,
        bucket: &str,
        key: &str,
        size: u64,
        metadata: &[(&str, &str)],
    ) -> Result<()> {
        // `storage_class` for a freshly-uploaded object. We only persist a
        // coarse STANDARD marker; the next re-list from S3 replaces it.
        let (prefix, _name) = object_prefix_and_name(bucket, key);
        let sc = if metadata.is_empty() { "" } else { "STANDARD" };
        self.conn
            .execute(
                "INSERT INTO objects (bucket, prefix, key, size, last_modified, is_folder, storage_class)
                 VALUES (?1, ?2, ?3, ?4, strftime('%s','now'), 0, ?5)
                 ON CONFLICT(bucket, prefix, key) DO UPDATE SET
                     size = excluded.size,
                     last_modified = excluded.last_modified,
                     is_folder = excluded.is_folder,
                     storage_class = excluded.storage_class",
                rusqlite::params![bucket, prefix, key, size as i64, sc],
            )
            .with_context(|| format!("failed to upsert uploaded object {key}"))?;
        Ok(())
    }

    /// Return the cached listing of `(bucket, prefix)`, or `None` when the
    /// cache has no knowledge of that folder yet.
    pub fn get_prefix(&self, bucket: &str, prefix: &str) -> Result<Option<Vec<ObjectInfo>>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT key, size, last_modified, is_folder, storage_class
                 FROM objects WHERE bucket = ?1 AND prefix = ?2 ORDER BY is_folder DESC, key COLLATE NOCASE",
            )
            .context("failed to prepare select query")?;
        let rows = stmt
            .query_map([bucket, prefix], row_to_object)
            .with_context(|| format!("failed to query s3://{bucket}/{prefix}"))?;
        let objects = rows
            .collect::<rusqlite::Result<Vec<_>>>()
            .context("failed to read cached rows")?;
        if objects.is_empty() {
            Ok(None)
        } else {
            Ok(Some(objects))
        }
    }
}

/// Split an object's full key into its folder `prefix` and the bare file name
/// stored in the cache (the cache keys a row by `(bucket, prefix, key)` where
/// `prefix` is the folder being listed).
fn object_prefix_and_name(_bucket: &str, key: &str) -> (String, String) {
    match key.rfind('/') {
        Some(pos) => (key[..=pos].to_string(), key[pos + 1..].to_string()),
        None => (String::new(), key.to_string()),
    }
}

/// True when a cached folder listing exactly matches a remote one, i.e. they
/// have the same objects (order-insensitive).
pub fn listings_match(local: &[ObjectInfo], remote: &[ObjectInfo]) -> bool {
    if local.len() != remote.len() {
        return false;
    }
    let local: HashSet<(&str, Option<u64>, i64)> = local
        .iter()
        .map(|o| {
            (
                o.key.as_str(),
                o.size,
                o.last_modified.map(|d| d.timestamp()).unwrap_or(0),
            )
        })
        .collect();
    remote.iter().all(|o| {
        local.contains(&(
            o.key.as_str(),
            o.size,
            o.last_modified.map(|d| d.timestamp()).unwrap_or(0),
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::StorageClass;
    use chrono::{TimeZone, Utc};

    fn object(key: &str, size: Option<u64>, modified: bool) -> ObjectInfo {
        ObjectInfo {
            key: key.to_string(),
            size,
            last_modified: if modified {
                Some(Utc.with_ymd_and_hms(2026, 1, 2, 3, 4, 5).unwrap())
            } else {
                None
            },
            is_folder: false,
            storage_class: StorageClass::Standard,
        }
    }

    fn temp_db(name: &str) -> ObjectCache {
        let dir = std::env::temp_dir().join(format!("s3tui-cache-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        ObjectCache::open(&dir.join("cache.db")).unwrap()
    }

    #[test]
    fn set_then_get_roundtrips() {
        let cache = temp_db("rt");
        let prefix = "folder/";
        let objects = vec![object("folder/one.txt", Some(10), true)];
        cache.set_prefix("b", prefix, &objects).unwrap();
        let got = cache.get_prefix("b", prefix).unwrap().unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].key, "folder/one.txt");
        assert_eq!(got[0].size, Some(10));
    }

    #[test]
    fn prune_removes_keys_not_present_remotely() {
        let cache = temp_db("prune");
        let prefix = "";
        cache
            .set_prefix("b", prefix, &[object("a.txt", Some(1), false)])
            .unwrap();
        cache
            .set_prefix("b", prefix, &[object("b.txt", Some(2), false)])
            .unwrap();
        let got = cache.get_prefix("b", prefix).unwrap().unwrap();
        assert_eq!(
            got.iter().map(|o| o.key.as_str()).collect::<Vec<_>>(),
            vec!["b.txt"]
        );
    }

    #[test]
    fn upload_is_added_and_listed() {
        let cache = temp_db("up");
        let prefix = "dir/";
        cache
            .upsert_object("b", "dir/file.log", 42, &[("env", "prod")])
            .unwrap();
        let got = cache.get_prefix("b", prefix).unwrap().unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].key, "dir/file.log");
    }

    #[test]
    fn get_prefix_returns_none_when_empty() {
        let cache = temp_db("none");
        assert!(cache.get_prefix("b", "").unwrap().is_none());
    }

    #[test]
    fn empty_listings_are_still_marked() {
        let cache = temp_db("empty");
        assert!(!cache.listing_exists("b", "root/").unwrap());
        cache.set_prefix("b", "root/", &[]).unwrap();
        assert!(cache.listing_exists("b", "root/").unwrap());
        assert!(cache.get_prefix("b", "root/").unwrap().is_none());
    }

    #[test]
    fn listings_match_is_order_and_content_aware() {
        let a = vec![object("x", Some(1), true), object("y", Some(2), true)];
        let b = vec![object("y", Some(2), true), object("x", Some(1), true)];
        assert!(listings_match(&a, &b));
        let mut changed = b.clone();
        changed[0].size = Some(99);
        assert!(!listings_match(&a, &changed));
    }
}
