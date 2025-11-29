use rusqlite::{params, Connection, Result};
use std::path::Path;

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
                size INTEGER
            )",
            [],
        )?;
        Ok(())
    }

    pub fn upsert_file_state(
        &self,
        source_path: &str,
        dest_path: &str,
        created: i64,
        changed: i64,
        modified: i64,
        permissions: u32,
        hash: Option<&str>,
        size: u64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO files (
                source_path, dest_path, created_date, changed_date, modified_date, permissions, hash, size
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                source_path,
                dest_path,
                created,
                changed,
                modified,
                permissions,
                hash,
                size
            ],
        )?;
        Ok(())
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
