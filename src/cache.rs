use anyhow::{Context, Result};
use rusqlite::Connection;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::app::ObjectInfo;

pub const CACHE_DIR_NAME: &str = "s3-tui";
pub const CACHE_DB_NAME: &str = "cache.db";

pub fn cache_db_path() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(".cache")
        .join(CACHE_DIR_NAME)
        .join(CACHE_DB_NAME)
}

/// Writable handle over the SQLite object cache.
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
    pub fn open_default() -> Result<Self> {
        let path = cache_db_path();
        Self::open(&path)
    }

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
    /// pruning stale rows and upserting current ones. Returns `true` when the
    /// cache had a different listing and was overwritten.
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

    /// Update the cached listing only when it differs from the remote one.
    pub fn sync_listing(&self, bucket: &str, prefix: &str, remote: &[ObjectInfo]) -> Result<()> {
        match self.get_prefix(bucket, prefix)? {
            Some(local) if listings_match(&local, remote) => Ok(()),
            _ => self.set_prefix(bucket, prefix, remote),
        }
    }

    /// `true` when the cache has a recorded listing for `(bucket, prefix)`.
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

    /// Record a freshly-uploaded object until the next re-list refreshes it
    /// from S3.
    pub fn upsert_object(
        &self,
        bucket: &str,
        key: &str,
        size: u64,
        metadata: &[(&str, &str)],
    ) -> Result<()> {
        let prefix = object_prefix(key);
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

    /// Cached listing of `(bucket, prefix)`, or `None` when unknown.
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

/// Folder prefix of an object's full key, including the trailing slash when
/// the key lives in a folder. The cache keys rows by `(bucket, prefix, key)`.
fn object_prefix(key: &str) -> String {
    match key.rfind('/') {
        Some(pos) => key[..=pos].to_string(),
        None => String::new(),
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

    #[test]
    fn sync_listing_skips_identical_remotes() {
        let cache = temp_db("sync");
        cache
            .set_prefix("b", "", &[object("a.txt", Some(1), true)])
            .unwrap();
        cache
            .sync_listing("b", "", &[object("a.txt", Some(1), true)])
            .unwrap();
        let mut remote = vec![object("b.txt", Some(2), true)];
        cache.sync_listing("b", "", &remote).unwrap();
        remote.clear();
        let got = cache.get_prefix("b", "").unwrap().unwrap();
        assert_eq!(got[0].key, "b.txt");
    }
}
