use rusqlite::{params, Connection, Result};
use std::path::Path;

/// File sync status in the database
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Pending, // Needs to be transferred
    Synced,  // Already transferred or confirmed in-sync
}

impl FileStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            FileStatus::Pending => "pending",
            FileStatus::Synced => "synced",
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s {
            "synced" => FileStatus::Synced,
            _ => FileStatus::Pending,
        }
    }
}

/// Represents a file record from the database
#[derive(Debug, Clone)]
pub struct FileRecord {
    pub source_path: String,
    pub dest_path: String,
    pub modified_date: i64,
    pub size: u64,
    pub status: FileStatus,
    pub atime: i64,
    pub ctime: i64,
    pub permissions: u32,
    pub hash: Option<String>,
}

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        let conn = Connection::open(path)?;
        Self::init(&conn)?;
        Ok(Self { conn })
    }

    fn init(conn: &Connection) -> Result<()> {
        conn.execute(
            "CREATE TABLE IF NOT EXISTS files (
                source_path TEXT PRIMARY KEY,
                dest_path TEXT NOT NULL,
                created_date INTEGER,
                changed_date INTEGER,
                modified_date INTEGER,
                permissions INTEGER,
                hash TEXT,
                size INTEGER,
                status TEXT NOT NULL DEFAULT 'pending'
            )",
            [],
        )?;
        // Index on status for efficient pending queries (count, sum, list)
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_files_status ON files(status)",
            [],
        )?;
        Ok(())
    }

    /// Insert or update a file record, preserving hash if file hasn't changed
    pub fn upsert_file(
        &self,
        source_path: &str,
        dest_path: &str,
        created: i64,
        changed: i64,
        modified: i64,
        permissions: u32,
        size: u64,
        status: FileStatus,
    ) -> Result<()> {
        // Check if file exists with same mtime and size - if so, preserve hash
        let existing_hash: Option<String> = self
            .conn
            .query_row(
                "SELECT hash FROM files WHERE source_path = ?1 AND modified_date = ?2 AND size = ?3",
                params![source_path, modified, size],
                |row| row.get(0),
            )
            .ok()
            .flatten();

        self.conn.execute(
            "INSERT OR REPLACE INTO files (
                source_path, dest_path, created_date, changed_date, modified_date,
                permissions, hash, size, status
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                source_path,
                dest_path,
                created,
                changed,
                modified,
                permissions,
                existing_hash,
                size,
                status.as_str()
            ],
        )?;
        Ok(())
    }

    /// Mark a file as synced and store its hash
    pub fn mark_synced(&self, source_path: &str, hash: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE files SET status = 'synced', hash = ?2 WHERE source_path = ?1",
            params![source_path, hash],
        )?;
        Ok(())
    }

    /// Get count of pending files in the backlog
    pub fn pending_count(&self) -> Result<u64> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM files WHERE status = 'pending'",
            [],
            |row| row.get(0),
        )?;
        Ok(count as u64)
    }

    /// Get total bytes of all pending files in the backlog
    pub fn pending_total_bytes(&self) -> Result<u64> {
        let total: i64 = self.conn.query_row(
            "SELECT COALESCE(SUM(size), 0) FROM files WHERE status = 'pending'",
            [],
            |row| row.get(0),
        )?;
        Ok(total as u64)
    }

    /// Get all pending files (the backlog)
    pub fn get_pending_files(&self) -> Result<Vec<FileRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT source_path, dest_path, modified_date, size, status,
                    changed_date, created_date, permissions, hash
             FROM files WHERE status = 'pending'",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(FileRecord {
                source_path: row.get(0)?,
                dest_path: row.get(1)?,
                modified_date: row.get(2)?,
                size: row.get::<_, i64>(3)? as u64,
                status: FileStatus::from_str(&row.get::<_, String>(4)?),
                ctime: row.get(5)?,
                atime: row.get(6)?,
                permissions: row.get(7)?,
                hash: row.get(8)?,
            })
        })?;

        let mut files = Vec::new();
        for row in rows {
            files.push(row?);
        }
        Ok(files)
    }

    /// Check if database has any records
    pub fn has_records(&self) -> Result<bool> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))?;
        Ok(count > 0)
    }

    /// Remove files from DB that are no longer in source (for cleanup phase)
    pub fn get_all_dest_paths(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare("SELECT dest_path FROM files")?;
        let rows = stmt.query_map([], |row| row.get(0))?;

        let mut paths = Vec::new();
        for row in rows {
            paths.push(row?);
        }
        Ok(paths)
    }

    pub fn get_file_hash(&self, source_path: &str) -> Result<Option<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT hash FROM files WHERE source_path = ?1")?;
        let mut rows = stmt.query(params![source_path])?;

        if let Some(row) = rows.next()? {
            Ok(row.get(0)?)
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_db_init_and_ops() -> Result<()> {
        let db = Database::new(":memory:")?;

        db.upsert_file(
            "/src/file1",
            "/dest/file1",
            100,
            200,
            300,
            0o644,
            1024,
            FileStatus::Pending,
        )?;

        assert_eq!(db.pending_count()?, 1);

        // Mark as synced
        db.mark_synced("/src/file1", "abc123hash")?;

        assert_eq!(db.pending_count()?, 0);

        let hash = db.get_file_hash("/src/file1")?;
        assert_eq!(hash, Some("abc123hash".to_string()));

        Ok(())
    }

    #[test]
    fn test_hash_preservation() -> Result<()> {
        let db = Database::new(":memory:")?;

        // Insert file and mark synced with hash
        db.upsert_file(
            "/src/file1",
            "/dest/file1",
            100,
            200,
            300,
            0o644,
            1024,
            FileStatus::Pending,
        )?;
        db.mark_synced("/src/file1", "originalhash")?;

        // Re-upsert with same mtime and size - hash should be preserved
        db.upsert_file(
            "/src/file1",
            "/dest/file1",
            100,
            200,
            300, // Same mtime
            0o644,
            1024, // Same size
            FileStatus::Synced,
        )?;

        let hash = db.get_file_hash("/src/file1")?;
        assert_eq!(hash, Some("originalhash".to_string()));

        // Re-upsert with different mtime - hash should be cleared
        db.upsert_file(
            "/src/file1",
            "/dest/file1",
            100,
            200,
            400, // Different mtime
            0o644,
            1024,
            FileStatus::Pending,
        )?;

        let hash = db.get_file_hash("/src/file1")?;
        assert_eq!(hash, None);

        // Re-insert and mark synced again
        db.upsert_file(
            "/src/file1",
            "/dest/file1",
            100,
            200,
            400,
            0o644,
            1024,
            FileStatus::Pending,
        )?;
        db.mark_synced("/src/file1", "newhash")?;

        // Re-upsert with different size - hash should be cleared
        db.upsert_file(
            "/src/file1",
            "/dest/file1",
            100,
            200,
            400, // Same mtime
            0o644,
            2048, // Different size
            FileStatus::Pending,
        )?;

        let hash = db.get_file_hash("/src/file1")?;
        assert_eq!(hash, None);

        Ok(())
    }

    #[test]
    fn test_get_pending_files() -> Result<()> {
        let db = Database::new(":memory:")?;

        db.upsert_file(
            "/src/file1",
            "/dest/file1",
            100,
            200,
            300,
            0o644,
            1024,
            FileStatus::Pending,
        )?;
        db.upsert_file(
            "/src/file2",
            "/dest/file2",
            100,
            200,
            300,
            0o644,
            2048,
            FileStatus::Synced,
        )?;
        db.upsert_file(
            "/src/file3",
            "/dest/file3",
            100,
            200,
            300,
            0o644,
            512,
            FileStatus::Pending,
        )?;

        let pending = db.get_pending_files()?;
        assert_eq!(pending.len(), 2);

        Ok(())
    }

    #[test]
    fn test_pending_total_bytes() -> Result<()> {
        let db = Database::new(":memory:")?;

        // Empty database should return 0
        assert_eq!(db.pending_total_bytes()?, 0);

        db.upsert_file(
            "/src/file1",
            "/dest/file1",
            100,
            200,
            300,
            0o644,
            1024,
            FileStatus::Pending,
        )?;
        db.upsert_file(
            "/src/file2",
            "/dest/file2",
            100,
            200,
            300,
            0o644,
            2048,
            FileStatus::Synced, // Should not be counted
        )?;
        db.upsert_file(
            "/src/file3",
            "/dest/file3",
            100,
            200,
            300,
            0o644,
            512,
            FileStatus::Pending,
        )?;

        // Only pending files: 1024 + 512 = 1536
        assert_eq!(db.pending_total_bytes()?, 1536);

        Ok(())
    }
}
