//! SQLite persistence for likes and downloads

use std::path::PathBuf;

use sqlx::sqlite::{SqliteConnectOptions, SqlitePool};
use sqlx::{ConnectOptions, Row};

/// Persistent storage for liked tracks and download registry
pub struct MonoStorage {
    pool: SqlitePool,
}

impl MonoStorage {
    /// Open (or create) the SQLite database at `db_path`.
    pub async fn new(db_path: PathBuf) -> Result<Self, String> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("failed to create db dir: {e}"))?;
        }
        let db_url = format!("sqlite:{}?mode=rwc", db_path.display());
        let opts = db_url
            .parse::<SqliteConnectOptions>()
            .map_err(|e| format!("bad db url: {e}"))?
            .disable_statement_logging();
        let pool = SqlitePool::connect_with(opts)
            .await
            .map_err(|e| format!("db connect failed: {e}"))?;
        let storage = Self { pool };
        storage.run_migrations().await?;
        Ok(storage)
    }

    #[allow(clippy::too_many_lines)]
    async fn run_migrations(&self) -> Result<(), String> {
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS likes (
                track_id TEXT PRIMARY KEY,
                created_at INTEGER NOT NULL
            )",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| format!("migration failed: {e}"))?;

        // Add source column to likes (idempotent — only ignore "duplicate column" errors)
        if let Err(e) = sqlx::query("ALTER TABLE likes ADD COLUMN source TEXT")
            .execute(&self.pool)
            .await
        {
            let msg = e.to_string();
            if !msg.contains("duplicate column") {
                return Err(format!("migration failed (alter likes): {msg}"));
            }
        }

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS downloads (
                track_id TEXT PRIMARY KEY,
                local_path TEXT NOT NULL,
                title TEXT,
                artist TEXT,
                album TEXT,
                quality TEXT,
                created_at INTEGER NOT NULL
            )",
        )
        .execute(&self.pool)
        .await
        .map_err(|e| format!("migration failed: {e}"))?;

        // Migrate tables from INTEGER to TEXT track_ids if needed.
        // SQLite doesn't support ALTER COLUMN, so we rename → create → copy → drop.
        // Check if migration is needed by looking at the column type.
        let needs_migrate = sqlx::query(
            "SELECT type FROM pragma_table_info('likes') WHERE name = 'track_id'",
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| format!("migration check failed: {e}"))?
        .is_some_and(|row| {
            let col_type: String = row.get("type");
            col_type.to_uppercase() != "TEXT"
        });

        if needs_migrate {
            tracing::info!("migrating likes table: INTEGER → TEXT track_ids");
            sqlx::query("ALTER TABLE likes RENAME TO likes_old")
                .execute(&self.pool).await
                .map_err(|e| format!("migration failed: {e}"))?;
            sqlx::query(
                "CREATE TABLE likes (
                    track_id TEXT PRIMARY KEY,
                    created_at INTEGER NOT NULL,
                    source TEXT
                )",
            )
            .execute(&self.pool).await
            .map_err(|e| format!("migration failed: {e}"))?;
            sqlx::query(
                "INSERT INTO likes (track_id, created_at, source)
                 SELECT CAST(track_id AS TEXT), created_at, source FROM likes_old",
            )
            .execute(&self.pool).await
            .map_err(|e| format!("migration failed: {e}"))?;
            sqlx::query("DROP TABLE likes_old")
                .execute(&self.pool).await
                .map_err(|e| format!("migration failed: {e}"))?;
        }

        let needs_migrate_dl = sqlx::query(
            "SELECT type FROM pragma_table_info('downloads') WHERE name = 'track_id'",
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| format!("migration check failed: {e}"))?
        .is_some_and(|row| {
            let col_type: String = row.get("type");
            col_type.to_uppercase() != "TEXT"
        });

        if needs_migrate_dl {
            tracing::info!("migrating downloads table: INTEGER → TEXT track_ids");
            sqlx::query("ALTER TABLE downloads RENAME TO downloads_old")
                .execute(&self.pool).await
                .map_err(|e| format!("migration failed: {e}"))?;
            sqlx::query(
                "CREATE TABLE downloads (
                    track_id TEXT PRIMARY KEY,
                    local_path TEXT NOT NULL,
                    title TEXT,
                    artist TEXT,
                    album TEXT,
                    quality TEXT,
                    created_at INTEGER NOT NULL
                )",
            )
            .execute(&self.pool).await
            .map_err(|e| format!("migration failed: {e}"))?;
            sqlx::query(
                "INSERT INTO downloads (track_id, local_path, title, artist, album, quality, created_at)
                 SELECT CAST(track_id AS TEXT), local_path, title, artist, album, quality, created_at FROM downloads_old",
            )
            .execute(&self.pool).await
            .map_err(|e| format!("migration failed: {e}"))?;
            sqlx::query("DROP TABLE downloads_old")
                .execute(&self.pool).await
                .map_err(|e| format!("migration failed: {e}"))?;
        }

        Ok(())
    }

    // ── Likes ────────────────────────────────────────────────────────────

    /// Toggle like state. Returns the new liked state (true = now liked).
    pub async fn toggle_like(&self, track_id: &str, source: Option<String>) -> Result<bool, String> {
        let exists = self.is_liked(track_id).await?;
        if exists {
            sqlx::query("DELETE FROM likes WHERE track_id = ?")
                .bind(track_id)
                .execute(&self.pool)
                .await
                .map_err(|e| format!("unlike failed: {e}"))?;
            Ok(false)
        } else {
            let now = chrono::Utc::now().timestamp();
            sqlx::query("INSERT INTO likes (track_id, created_at, source) VALUES (?, ?, ?)")
                .bind(track_id)
                .bind(now)
                .bind(source.as_deref())
                .execute(&self.pool)
                .await
                .map_err(|e| format!("like failed: {e}"))?;
            Ok(true)
        }
    }

    /// Check if a track is liked.
    pub async fn is_liked(&self, track_id: &str) -> Result<bool, String> {
        let row = sqlx::query("SELECT COUNT(*) as cnt FROM likes WHERE track_id = ?")
            .bind(track_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| format!("is_liked query failed: {e}"))?;
        let cnt: i32 = row.get("cnt");
        Ok(cnt > 0)
    }

    /// Get all liked track IDs, ordered by most recently liked first.
    pub async fn liked_ids(&self) -> Result<Vec<String>, String> {
        let rows = sqlx::query("SELECT track_id FROM likes ORDER BY created_at DESC")
            .fetch_all(&self.pool)
            .await
            .map_err(|e| format!("liked_ids query failed: {e}"))?;
        Ok(rows
            .iter()
            .map(|r| r.get::<String, _>("track_id"))
            .collect())
    }

    /// Get all liked track IDs with their source annotation, ordered by most recently liked first.
    pub async fn liked_ids_with_source(&self) -> Result<Vec<(String, Option<String>)>, String> {
        let rows = sqlx::query("SELECT track_id, source FROM likes ORDER BY created_at DESC")
            .fetch_all(&self.pool)
            .await
            .map_err(|e| format!("liked_ids_with_source query failed: {e}"))?;
        Ok(rows
            .iter()
            .map(|r| {
                let id = r.get::<String, _>("track_id");
                let source: Option<String> = r.get("source");
                (id, source)
            })
            .collect())
    }

    // ── Downloads ────────────────────────────────────────────────────────

    /// Register a downloaded track.
    pub async fn register_download(
        &self,
        track_id: &str,
        path: &str,
        title: Option<&str>,
        artist: Option<&str>,
        album: Option<&str>,
        quality: Option<&str>,
    ) -> Result<(), String> {
        let now = chrono::Utc::now().timestamp();
        sqlx::query(
            "INSERT OR REPLACE INTO downloads (track_id, local_path, title, artist, album, quality, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(track_id)
        .bind(path)
        .bind(title)
        .bind(artist)
        .bind(album)
        .bind(quality)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| format!("register_download failed: {e}"))?;
        Ok(())
    }

    /// Get the local file path for a downloaded track.
    pub async fn get_download_path(&self, track_id: &str) -> Result<Option<String>, String> {
        let row = sqlx::query("SELECT local_path FROM downloads WHERE track_id = ?")
            .bind(track_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| format!("get_download_path failed: {e}"))?;
        Ok(row.map(|r| r.get("local_path")))
    }

    /// Delete a downloaded track (remove DB row + file on disk).
    pub async fn delete_download(&self, track_id: &str) -> Result<Option<String>, String> {
        let row = sqlx::query("SELECT local_path FROM downloads WHERE track_id = ?")
            .bind(track_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| format!("delete_download query failed: {e}"))?;
        let path = row.map(|r| r.get::<String, _>("local_path"));
        sqlx::query("DELETE FROM downloads WHERE track_id = ?")
            .bind(track_id)
            .execute(&self.pool)
            .await
            .map_err(|e| format!("delete_download failed: {e}"))?;
        if let Some(ref p) = path {
            let file_path = std::path::Path::new(p);
            std::fs::remove_file(file_path).ok();
            // Prune empty parent dirs up to the music root
            let music_root = dirs::home_dir().unwrap_or_default().join("Music/mono-tray");
            let mut dir = file_path.parent();
            while let Some(d) = dir {
                if d <= music_root.as_path() {
                    break;
                }
                if std::fs::read_dir(d)
                    .map(|mut r| r.next().is_none())
                    .unwrap_or(false)
                {
                    std::fs::remove_dir(d).ok();
                    dir = d.parent();
                } else {
                    break;
                }
            }
        }
        Ok(path)
    }

    /// Check if a track has been downloaded.
    pub async fn is_downloaded(&self, track_id: &str) -> Result<bool, String> {
        let row = sqlx::query("SELECT COUNT(*) as cnt FROM downloads WHERE track_id = ?")
            .bind(track_id)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| format!("is_downloaded query failed: {e}"))?;
        let cnt: i32 = row.get("cnt");
        Ok(cnt > 0)
    }
}
