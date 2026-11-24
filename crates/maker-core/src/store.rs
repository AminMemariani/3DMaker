//! SQLite persistence for project metadata. Large assets live on disk; the
//! database stores their paths and checksums. Secrets are never written here.
//!
//! The schema still carries a few columns from the image-to-3D version of
//! the app (subject, seed, steps, reference image, remote job). They are left
//! in place so existing libraries open unchanged, and are no longer written.

use crate::model::{ProjectState, StageKind, UserError};
use crate::ply::{Bounds, GaussianSummary};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
use std::sync::Mutex;

pub const SCHEMA_VERSION: i64 = 2;

const MIGRATIONS: &[&str] = &[
    // v1
    r#"
    CREATE TABLE projects (
        id TEXT PRIMARY KEY,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL,
        prompt TEXT NOT NULL,
        subject TEXT,
        provider TEXT NOT NULL,
        state TEXT NOT NULL,
        seed INTEGER,
        image_steps INTEGER,
        recon_steps INTEGER,
        reference_path TEXT,
        splat_path TEXT,
        splat_bytes INTEGER,
        splat_sha256 TEXT,
        splat_count INTEGER,
        sh_degree INTEGER,
        bounds_json TEXT,
        thumb_path TEXT,
        error_json TEXT,
        client_request_id TEXT UNIQUE
    );
    CREATE TABLE stages (
        project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
        kind TEXT NOT NULL,
        attempt INTEGER NOT NULL,
        status TEXT NOT NULL,
        model TEXT NOT NULL,
        version TEXT,
        seed INTEGER,
        remote_json TEXT,
        submitted_at INTEGER,
        finished_at INTEGER,
        error_json TEXT,
        PRIMARY KEY (project_id, kind, attempt)
    );
    CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
    "#,
    // v2: viewer orientation correction per project
    r#"
    ALTER TABLE projects ADD COLUMN view_rotation_json TEXT;
    CREATE INDEX projects_created ON projects(created_at DESC);
    "#,
];

/// Design-request lifecycle as persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageStatus {
    /// The request was sent and no answer has been recorded yet.
    Running,
    /// The answer was received and saved as a design.
    Done,
    Failed,
    Cancelled,
    /// Written by an older version; treated as finished-without-result.
    Unknown,
}

impl StageStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Unknown => "unknown",
        }
    }
    pub fn parse(s: &str) -> Self {
        match s {
            "running" => Self::Running,
            "done" => Self::Done,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProjectRow {
    pub id: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub prompt: String,
    pub provider: String,
    pub state: ProjectState,
    pub splat_path: Option<String>,
    pub splat_bytes: Option<u64>,
    pub splat_sha256: Option<String>,
    pub splat_count: Option<u64>,
    pub sh_degree: Option<u8>,
    pub bounds: Option<Bounds>,
    pub thumb_path: Option<String>,
    pub error: Option<UserError>,
    pub client_request_id: Option<String>,
    /// Viewer-only orientation correction (XYZ Euler degrees).
    pub view_rotation: [f32; 3],
}

/// Rows from before v2 came from producers that write y-down splats.
const LEGACY_VIEW_ROTATION: [f32; 3] = [180.0, 0.0, 0.0];

impl ProjectRow {
    pub fn new(id: String, prompt: String, provider: &str) -> Self {
        let now = crate::model::now_unix();
        Self {
            id,
            created_at: now,
            updated_at: now,
            prompt,
            provider: provider.into(),
            state: ProjectState::Queued,
            splat_path: None,
            splat_bytes: None,
            splat_sha256: None,
            splat_count: None,
            sh_degree: None,
            bounds: None,
            thumb_path: None,
            error: None,
            client_request_id: None,
            // Designs are built y-up, matching the viewer.
            view_rotation: [0.0; 3],
        }
    }
}

#[derive(Debug, Clone)]
pub struct StageRow {
    pub project_id: String,
    pub kind: StageKind,
    pub attempt: i64,
    pub status: StageStatus,
    pub model: String,
    pub submitted_at: Option<i64>,
    pub error: Option<UserError>,
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("database is from a newer version of 3D Maker (schema {0})")]
    TooNew(i64),
    #[error("not found")]
    NotFound,
}

pub struct Store {
    conn: Mutex<Connection>,
}

fn json<T: serde::Serialize>(v: &Option<T>) -> Option<String> {
    v.as_ref().and_then(|v| serde_json::to_string(v).ok())
}

fn from_json<T: serde::de::DeserializeOwned>(s: Option<String>) -> Option<T> {
    s.and_then(|s| serde_json::from_str(&s).ok())
}

impl Store {
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self, StoreError> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self, StoreError> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "FULL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        migrate(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn schema_version(&self) -> i64 {
        self.conn
            .lock()
            .unwrap()
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap_or(0)
    }

    pub fn insert_project(&self, p: &ProjectRow) -> Result<(), StoreError> {
        let c = self.conn.lock().unwrap();
        c.execute(
            "INSERT INTO projects (id, created_at, updated_at, prompt, provider, state, splat_path, splat_bytes, splat_sha256,
                splat_count, sh_degree, bounds_json, thumb_path, error_json, client_request_id, view_rotation_json)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
            params![
                p.id, p.created_at, p.updated_at, p.prompt, p.provider, p.state.as_str(), p.splat_path,
                p.splat_bytes.map(|v| v as i64), p.splat_sha256, p.splat_count.map(|v| v as i64), p.sh_degree,
                json(&p.bounds), p.thumb_path, json(&p.error), p.client_request_id, serde_json::to_string(&p.view_rotation).ok()
            ],
        )?;
        Ok(())
    }

    fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<ProjectRow> {
        Ok(ProjectRow {
            id: r.get("id")?,
            created_at: r.get("created_at")?,
            updated_at: r.get("updated_at")?,
            prompt: r.get("prompt")?,
            provider: r.get("provider")?,
            state: ProjectState::parse(&r.get::<_, String>("state")?),
            splat_path: r.get("splat_path")?,
            splat_bytes: r.get::<_, Option<i64>>("splat_bytes")?.map(|v| v as u64),
            splat_sha256: r.get("splat_sha256")?,
            splat_count: r.get::<_, Option<i64>>("splat_count")?.map(|v| v as u64),
            sh_degree: r.get("sh_degree")?,
            bounds: from_json(r.get("bounds_json")?),
            thumb_path: r.get("thumb_path")?,
            error: from_json(r.get("error_json")?),
            client_request_id: r.get("client_request_id")?,
            view_rotation: from_json(r.get("view_rotation_json")?).unwrap_or(LEGACY_VIEW_ROTATION),
        })
    }

    pub fn project(&self, id: &str) -> Result<ProjectRow, StoreError> {
        let c = self.conn.lock().unwrap();
        c.query_row("SELECT * FROM projects WHERE id = ?1", [id], Self::row)
            .optional()?
            .ok_or(StoreError::NotFound)
    }

    pub fn project_by_request(
        &self,
        client_request_id: &str,
    ) -> Result<Option<ProjectRow>, StoreError> {
        let c = self.conn.lock().unwrap();
        Ok(c.query_row(
            "SELECT * FROM projects WHERE client_request_id = ?1",
            [client_request_id],
            Self::row,
        )
        .optional()?)
    }

    pub fn projects(&self) -> Result<Vec<ProjectRow>, StoreError> {
        let c = self.conn.lock().unwrap();
        let mut s =
            c.prepare("SELECT * FROM projects ORDER BY created_at DESC, rowid DESC LIMIT 500")?;
        let rows = s.query_map([], Self::row)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn set_state(
        &self,
        id: &str,
        state: ProjectState,
        error: Option<&UserError>,
    ) -> Result<(), StoreError> {
        let c = self.conn.lock().unwrap();
        c.execute(
            "UPDATE projects SET state = ?2, error_json = ?3, updated_at = ?4 WHERE id = ?1",
            params![
                id,
                state.as_str(),
                error.and_then(|e| serde_json::to_string(e).ok()),
                crate::model::now_unix()
            ],
        )?;
        Ok(())
    }

    pub fn set_thumb(&self, id: &str, path: &str) -> Result<(), StoreError> {
        self.conn.lock().unwrap().execute(
            "UPDATE projects SET thumb_path = ?2 WHERE id = ?1",
            params![id, path],
        )?;
        Ok(())
    }

    /// Marks the project ready together with the validated splat, atomically.
    pub fn mark_ready(&self, id: &str, path: &str, s: &GaussianSummary) -> Result<(), StoreError> {
        let c = self.conn.lock().unwrap();
        c.execute(
            "UPDATE projects SET state = 'ready', error_json = NULL, splat_path = ?2, splat_bytes = ?3, splat_sha256 = ?4,
                splat_count = ?5, sh_degree = ?6, bounds_json = ?7, updated_at = ?8 WHERE id = ?1",
            params![id, path, s.file_bytes as i64, s.sha256, s.count as i64, s.sh_degree, serde_json::to_string(&s.bounds).ok(), crate::model::now_unix()],
        )?;
        Ok(())
    }

    pub fn delete_project(&self, id: &str) -> Result<(), StoreError> {
        self.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM projects WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Records a design request *before* it is sent.
    pub fn begin_stage(
        &self,
        project: &str,
        kind: StageKind,
        model: &str,
    ) -> Result<i64, StoreError> {
        let mut c = self.conn.lock().unwrap();
        let tx = c.transaction()?;
        let attempt: i64 = tx.query_row(
            "SELECT COALESCE(MAX(attempt), 0) + 1 FROM stages WHERE project_id = ?1 AND kind = ?2",
            params![project, kind.as_str()],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO stages (project_id, kind, attempt, status, model, submitted_at) VALUES (?1,?2,?3,'running',?4,?5)",
            params![project, kind.as_str(), attempt, model, crate::model::now_unix()],
        )?;
        tx.commit()?;
        Ok(attempt)
    }

    pub fn update_stage(
        &self,
        project: &str,
        kind: StageKind,
        attempt: i64,
        status: StageStatus,
        error: Option<&UserError>,
    ) -> Result<(), StoreError> {
        let c = self.conn.lock().unwrap();
        let finished = (status != StageStatus::Running).then(crate::model::now_unix);
        c.execute(
            "UPDATE stages SET status = ?4, error_json = ?5, finished_at = COALESCE(?6, finished_at)
             WHERE project_id = ?1 AND kind = ?2 AND attempt = ?3",
            params![
                project,
                kind.as_str(),
                attempt,
                status.as_str(),
                error.and_then(|e| serde_json::to_string(e).ok()),
                finished
            ],
        )?;
        Ok(())
    }

    /// Rows whose kind this version doesn't know (older image/3D stages) are skipped.
    fn stage_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Option<StageRow>> {
        let Some(kind) = StageKind::parse(&r.get::<_, String>("kind")?) else {
            return Ok(None);
        };
        Ok(Some(StageRow {
            project_id: r.get("project_id")?,
            kind,
            attempt: r.get("attempt")?,
            status: StageStatus::parse(&r.get::<_, String>("status")?),
            model: r.get("model")?,
            submitted_at: r.get("submitted_at")?,
            error: from_json(r.get("error_json")?),
        }))
    }

    pub fn latest_stage(
        &self,
        project: &str,
        kind: StageKind,
    ) -> Result<Option<StageRow>, StoreError> {
        let c = self.conn.lock().unwrap();
        Ok(c.query_row(
            "SELECT * FROM stages WHERE project_id = ?1 AND kind = ?2 ORDER BY attempt DESC LIMIT 1",
            params![project, kind.as_str()],
            Self::stage_row,
        )
        .optional()?
        .flatten())
    }

    pub fn unfinished_projects(&self) -> Result<Vec<ProjectRow>, StoreError> {
        let c = self.conn.lock().unwrap();
        let mut s = c.prepare(
            "SELECT * FROM projects WHERE state NOT IN ('ready','failed','cancelled','needsAttention') ORDER BY created_at DESC",
        )?;
        let rows = s.query_map([], Self::row)?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn setting(&self, key: &str) -> Option<String> {
        let c = self.conn.lock().unwrap();
        c.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
            r.get(0)
        })
        .optional()
        .ok()
        .flatten()
    }

    pub fn set_setting(&self, key: &str, value: Option<&str>) -> Result<(), StoreError> {
        let c = self.conn.lock().unwrap();
        match value {
            Some(v) => c.execute("INSERT INTO settings(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = ?2", params![key, v])?,
            None => c.execute("DELETE FROM settings WHERE key = ?1", [key])?,
        };
        Ok(())
    }
}

fn migrate(conn: &Connection) -> Result<(), StoreError> {
    let current: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if current > SCHEMA_VERSION {
        return Err(StoreError::TooNew(current));
    }
    for (i, sql) in MIGRATIONS.iter().enumerate() {
        let v = i as i64 + 1;
        if v > current {
            conn.execute_batch(&format!("BEGIN; {sql}; PRAGMA user_version = {v}; COMMIT;"))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_fresh_and_v1_databases() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.db");
        // Build a v1 database by hand, with a row from the old image/3D
        // pipeline, then reopen with the current code.
        {
            let c = Connection::open(&path).unwrap();
            c.execute_batch(&format!(
                "BEGIN; {}; PRAGMA user_version = 1; COMMIT;",
                MIGRATIONS[0]
            ))
            .unwrap();
            c.execute(
                "INSERT INTO projects (id, created_at, updated_at, prompt, provider, state, subject) VALUES ('a', 1, 1, 'old', 'replicate', 'ready', 'teapot')",
                [],
            )
            .unwrap();
            c.execute(
                "INSERT INTO stages (project_id, kind, attempt, status, model) VALUES ('a', 'splat', 1, 'remoteSucceeded', 'm')",
                [],
            )
            .unwrap();
        }
        let s = Store::open(&path).unwrap();
        assert_eq!(s.schema_version(), SCHEMA_VERSION);
        let p = s.project("a").unwrap();
        assert_eq!(p.prompt, "old");
        assert_eq!(p.state, ProjectState::Ready);
        assert_eq!(p.view_rotation, LEGACY_VIEW_ROTATION);
        // Old stage kinds are ignored rather than misread.
        assert!(s.latest_stage("a", StageKind::Design).unwrap().is_none());
        drop(s);
        // Re-opening is idempotent.
        assert_eq!(Store::open(&path).unwrap().schema_version(), SCHEMA_VERSION);
    }

    #[test]
    fn refuses_newer_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.db");
        Connection::open(&path)
            .unwrap()
            .pragma_update(None, "user_version", 99)
            .unwrap();
        assert!(matches!(Store::open(&path), Err(StoreError::TooNew(99))));
    }

    #[test]
    fn design_attempts_increment_and_finish() {
        let s = Store::open_in_memory().unwrap();
        s.insert_project(&ProjectRow::new("p".into(), "x".into(), "anthropic"))
            .unwrap();
        assert_eq!(s.project("p").unwrap().view_rotation, [0.0; 3]);
        let a1 = s.begin_stage("p", StageKind::Design, "m1").unwrap();
        let a2 = s.begin_stage("p", StageKind::Design, "m2").unwrap();
        assert_eq!((a1, a2), (1, 2));
        let st = s.latest_stage("p", StageKind::Design).unwrap().unwrap();
        assert_eq!((st.status, st.model.as_str()), (StageStatus::Running, "m2"));
        s.update_stage("p", StageKind::Design, a2, StageStatus::Done, None)
            .unwrap();
        assert_eq!(
            s.latest_stage("p", StageKind::Design)
                .unwrap()
                .unwrap()
                .status,
            StageStatus::Done
        );
    }
}
