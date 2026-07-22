use std::{path::Path, str::FromStr, time::Duration};

use atuin_common::utils;
use eyre::{Result, WrapErr};
use sqlx::{
    FromRow, SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};
use time::OffsetDateTime;
use tokio::fs;

use crate::{ClipboardEntry, ClipboardId};

#[derive(Clone, Debug, Default)]
pub struct SearchOptions {
    pub host: Option<String>,
    pub before: Option<OffsetDateTime>,
    pub after: Option<OffsetDateTime>,
    pub limit: Option<u32>,
    pub reverse: bool,
    pub include_deleted: bool,
}

#[derive(Clone, Debug)]
pub struct ClipboardDatabase {
    pool: SqlitePool,
}

#[derive(FromRow)]
struct ClipboardRow {
    id: String,
    timestamp: i64,
    content: String,
    content_hash: String,
    hostname: String,
    mime_type: String,
    deleted_at: Option<i64>,
}

impl TryFrom<ClipboardRow> for ClipboardEntry {
    type Error = eyre::Report;

    fn try_from(row: ClipboardRow) -> Result<Self> {
        Ok(Self {
            id: ClipboardId(row.id),
            timestamp: from_nanos(row.timestamp)?,
            content: row.content,
            content_hash: row.content_hash,
            hostname: row.hostname,
            mime_type: row.mime_type,
            deleted_at: row.deleted_at.map(from_nanos).transpose()?,
        })
    }
}

fn to_nanos(timestamp: OffsetDateTime) -> Result<i64> {
    timestamp
        .unix_timestamp_nanos()
        .try_into()
        .wrap_err("clipboard timestamp is outside SQLite's supported range")
}

fn from_nanos(timestamp: i64) -> Result<OffsetDateTime> {
    OffsetDateTime::from_unix_timestamp_nanos(i128::from(timestamp))
        .wrap_err("invalid clipboard timestamp in database")
}

impl ClipboardDatabase {
    pub async fn new(path: impl AsRef<Path>, timeout: f64) -> Result<Self> {
        let path = path.as_ref();
        tracing::debug!(path = ?path, "opening clipboard sqlite database");

        if utils::broken_symlink(path) {
            eyre::bail!(
                "clipboard database path is a broken symlink: {}",
                path.display()
            );
        }
        if !path.exists()
            && let Some(parent) = path.parent()
        {
            fs::create_dir_all(parent).await?;
        }

        let options = SqliteConnectOptions::from_str(
            path.as_os_str()
                .to_str()
                .ok_or_else(|| eyre::eyre!("clipboard database path is not valid UTF-8"))?,
        )?
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true)
        .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .acquire_timeout(Duration::from_secs_f64(timeout))
            .connect_with(options)
            .await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(Self { pool })
    }

    pub async fn insert(&self, entry: &ClipboardEntry) -> Result<()> {
        sqlx::query(
            "INSERT INTO clipboard_entries
             (id, timestamp, content, content_hash, hostname, mime_type, deleted_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )
        .bind(&entry.id.0)
        .bind(to_nanos(entry.timestamp)?)
        .bind(&entry.content)
        .bind(&entry.content_hash)
        .bind(&entry.hostname)
        .bind(&entry.mime_type)
        .bind(entry.deleted_at.map(to_nanos).transpose()?)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn insert_batch(&self, entries: &[ClipboardEntry]) -> Result<()> {
        let mut transaction = self.pool.begin().await?;
        for entry in entries {
            sqlx::query(
                "INSERT INTO clipboard_entries
                 (id, timestamp, content, content_hash, hostname, mime_type, deleted_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )
            .bind(&entry.id.0)
            .bind(to_nanos(entry.timestamp)?)
            .bind(&entry.content)
            .bind(&entry.content_hash)
            .bind(&entry.hostname)
            .bind(&entry.mime_type)
            .bind(entry.deleted_at.map(to_nanos).transpose()?)
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        Ok(())
    }

    pub async fn materialize(&self, entry: &ClipboardEntry) -> Result<()> {
        let tombstone: Option<i64> =
            sqlx::query_scalar("SELECT deleted_at FROM clipboard_tombstones WHERE id = ?1")
                .bind(&entry.id.0)
                .fetch_optional(&self.pool)
                .await?;
        let deleted_at = entry.deleted_at.map(to_nanos).transpose()?.or(tombstone);
        sqlx::query(
            "INSERT INTO clipboard_entries
             (id, timestamp, content, content_hash, hostname, mime_type, deleted_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET
                 timestamp = excluded.timestamp,
                 content = excluded.content,
                 content_hash = excluded.content_hash,
                 hostname = excluded.hostname,
                 mime_type = excluded.mime_type,
                 deleted_at = COALESCE(clipboard_entries.deleted_at, excluded.deleted_at)",
        )
        .bind(&entry.id.0)
        .bind(to_nanos(entry.timestamp)?)
        .bind(&entry.content)
        .bind(&entry.content_hash)
        .bind(&entry.hostname)
        .bind(&entry.mime_type)
        .bind(deleted_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn load(&self, id: &ClipboardId) -> Result<Option<ClipboardEntry>> {
        sqlx::query_as::<_, ClipboardRow>(
            "SELECT id, timestamp, content, content_hash, hostname, mime_type, deleted_at
             FROM clipboard_entries WHERE id = ?1",
        )
        .bind(&id.0)
        .fetch_optional(&self.pool)
        .await?
        .map(TryInto::try_into)
        .transpose()
    }

    pub async fn list(&self, options: &SearchOptions) -> Result<Vec<ClipboardEntry>> {
        self.search("", options).await
    }

    pub async fn search(
        &self,
        query: &str,
        options: &SearchOptions,
    ) -> Result<Vec<ClipboardEntry>> {
        let before = options.before.map(to_nanos).transpose()?;
        let after = options.after.map(to_nanos).transpose()?;
        let limit = i64::from(options.limit.unwrap_or(u32::MAX));
        let order = if options.reverse { "ASC" } else { "DESC" };
        let sql = format!(
            "SELECT id, timestamp, content, content_hash, hostname, mime_type, deleted_at
             FROM clipboard_entries
             WHERE instr(content, ?1) > 0
               AND (?2 IS NULL OR hostname = ?2)
               AND (?3 IS NULL OR timestamp < ?3)
               AND (?4 IS NULL OR timestamp > ?4)
               AND (?5 OR deleted_at IS NULL)
             ORDER BY timestamp {order}
             LIMIT ?6"
        );
        let rows = sqlx::query_as::<_, ClipboardRow>(sqlx::AssertSqlSafe(sql))
            .bind(query)
            .bind(options.host.as_deref())
            .bind(before)
            .bind(after)
            .bind(options.include_deleted)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub async fn soft_delete(&self, id: &ClipboardId, deleted_at: OffsetDateTime) -> Result<bool> {
        let deleted_at = to_nanos(deleted_at)?;
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO clipboard_tombstones (id, deleted_at) VALUES (?1, ?2)
             ON CONFLICT(id) DO UPDATE SET deleted_at = MAX(deleted_at, excluded.deleted_at)",
        )
        .bind(&id.0)
        .bind(deleted_at)
        .execute(&mut *transaction)
        .await?;
        let result = sqlx::query(
            "UPDATE clipboard_entries SET deleted_at = ?2
             WHERE id = ?1 AND deleted_at IS NULL",
        )
        .bind(&id.0)
        .bind(deleted_at)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn apply_remote_deletion(&self, id: &ClipboardId) -> Result<bool> {
        self.soft_delete(id, OffsetDateTime::now_utc()).await
    }

    pub async fn count_active(&self) -> Result<u64> {
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM clipboard_entries WHERE deleted_at IS NULL")
                .fetch_one(&self.pool)
                .await?;
        Ok(count.try_into()?)
    }

    pub async fn latest_active(&self) -> Result<Option<ClipboardEntry>> {
        self.list(&SearchOptions {
            limit: Some(1),
            ..SearchOptions::default()
        })
        .await
        .map(|mut entries| entries.pop())
    }

    pub async fn active_with_content(
        &self,
        content: &str,
        limit: u32,
    ) -> Result<Vec<ClipboardEntry>> {
        let rows = sqlx::query_as::<_, ClipboardRow>(
            "SELECT id, timestamp, content, content_hash, hostname, mime_type, deleted_at
             FROM clipboard_entries
             WHERE content = ?1 AND deleted_at IS NULL
             ORDER BY timestamp DESC
             LIMIT ?2",
        )
        .bind(content)
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(TryInto::try_into).collect()
    }

    pub async fn entries_older_than(
        &self,
        cutoff: OffsetDateTime,
        limit: u32,
    ) -> Result<Vec<ClipboardEntry>> {
        self.list(&SearchOptions {
            before: Some(cutoff),
            limit: Some(limit),
            ..SearchOptions::default()
        })
        .await
    }

    pub async fn clear_active(&self, deleted_at: OffsetDateTime) -> Result<Vec<ClipboardId>> {
        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM clipboard_entries WHERE deleted_at IS NULL ORDER BY timestamp DESC",
        )
        .fetch_all(&self.pool)
        .await?;
        sqlx::query("UPDATE clipboard_entries SET deleted_at = ?1 WHERE deleted_at IS NULL")
            .bind(to_nanos(deleted_at)?)
            .execute(&self.pool)
            .await?;
        Ok(ids.into_iter().map(ClipboardId).collect())
    }

    pub async fn prune_old(&self, cutoff: OffsetDateTime, limit: u32) -> Result<Vec<ClipboardId>> {
        let entries = self.entries_older_than(cutoff, limit).await?;
        let mut ids = Vec::with_capacity(entries.len());
        for entry in entries {
            self.soft_delete(&entry.id, OffsetDateTime::now_utc())
                .await?;
            ids.push(entry.id);
        }
        Ok(ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(content: &str, seconds: i64) -> ClipboardEntry {
        let mut entry = ClipboardEntry::new(content.to_owned(), "host".to_owned());
        entry.timestamp = OffsetDateTime::from_unix_timestamp(seconds).unwrap();
        entry
    }

    #[tokio::test]
    async fn migration_insert_search_delete_and_batch() {
        let db = ClipboardDatabase::new("sqlite::memory:", 1.0)
            .await
            .unwrap();
        let first = entry("hello\n世界", 1);
        let second = entry("other", 2);
        db.insert(&first).await.unwrap();
        db.insert_batch(std::slice::from_ref(&second))
            .await
            .unwrap();
        assert_eq!(db.load(&first.id).await.unwrap(), Some(first.clone()));
        assert_eq!(db.list(&SearchOptions::default()).await.unwrap()[0], second);
        let matches = db.search("世界", &SearchOptions::default()).await.unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0], first);
        assert!(
            db.search("%", &SearchOptions::default())
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            db.soft_delete(&first.id, OffsetDateTime::now_utc())
                .await
                .unwrap()
        );
        assert!(
            db.search("世界", &SearchOptions::default())
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(db.count_active().await.unwrap(), 1);
        assert!(db.insert(&first).await.is_err());
        assert_eq!(
            db.active_with_content("other", 250).await.unwrap(),
            vec![second]
        );
    }

    #[tokio::test]
    async fn persists_after_reopening() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clipboard.db");
        let saved = entry("persistent", 3);
        ClipboardDatabase::new(&path, 1.0)
            .await
            .unwrap()
            .insert(&saved)
            .await
            .unwrap();
        let reopened = ClipboardDatabase::new(&path, 1.0).await.unwrap();
        assert_eq!(reopened.load(&saved.id).await.unwrap(), Some(saved));
    }

    #[tokio::test]
    async fn deletion_before_create_remains_deleted() {
        let db = ClipboardDatabase::new("sqlite::memory:", 1.0)
            .await
            .unwrap();
        let entry = entry("late create", 4);
        assert!(!db.apply_remote_deletion(&entry.id).await.unwrap());
        db.materialize(&entry).await.unwrap();
        assert_eq!(db.count_active().await.unwrap(), 0);
        assert!(
            db.load(&entry.id)
                .await
                .unwrap()
                .unwrap()
                .deleted_at
                .is_some()
        );
    }
}
