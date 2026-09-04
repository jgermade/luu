//! Sessions on disk: a cache of the fold, never a second log.
//!
//! The design's rule for persistence is one sentence and it decides everything
//! here:
//!
//! > Whatever the store holds must be reproducible by folding the record.
//!
//! So a row holds [`SessionView`] — the structure `api.rs` already computes
//! from a recording and `luu export` already writes out — serialised whole,
//! and nothing else. The obvious alternative, a table per turn and task and
//! tool call, is **a second definition of the fold**: `api.rs` says what a turn
//! is, DDL would say it again, and the first time the two are changed apart the
//! store and the live server start disagreeing about a session. That is exactly
//! the drift the static mirror exists to avoid.
//!
//! The listing columns beside it are duplicated *out of* the view at write
//! time, so `GET /api/sessions` does not parse every blob to list them, and
//! they come from [`SessionView::summary`] rather than from anywhere else.
//! [`SessionStore::load`] answers from the blob, which is what makes the parity
//! test in `tests/store_parity.rs` able to catch a column that started carrying
//! something the fold does not have.
//!
//! Argued in `RECORD/2026-09-02.sessions-in-sqlite.completed.md`.

use std::path::{Path, PathBuf};

use agent_core::api::{SessionSummary, SessionView};
use anyhow::{Context, Result};
use rusqlite::Connection;

/// The shape of the schema. Stored in `PRAGMA user_version`, because a database
/// that cannot say which shape it is is one that has to be deleted the first
/// time the shape moves.
const SCHEMA: i32 = 1;

/// Where the store lives when nobody says otherwise.
///
/// `~/.loude/`, and deliberately not beside `luu.toml`. The two answer
/// different questions: the policy file describes *this project* and is meant
/// to be committed with it, while the store is *this machine's* history — and a
/// session store that travelled with a checkout would put one project's
/// conversation into every clone of it.
pub fn default_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").filter(|home| !home.is_empty())?;
    Some(PathBuf::from(home).join(".loude").join("sessions.db"))
}

pub struct SessionStore {
    connection: Connection,
}

impl SessionStore {
    /// Opens it, creating the file and the schema if they are not there.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let connection =
            Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        Self::prepare(connection)
    }

    /// The same, in memory. For tests, and for a run that wants the read side
    /// without leaving anything behind.
    pub fn in_memory() -> Result<Self> {
        Self::prepare(Connection::open_in_memory()?)
    }

    fn prepare(connection: Connection) -> Result<Self> {
        // A session worth resuming is usually one that ended badly, which is
        // the same argument the recorder makes for flushing per line.
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "foreign_keys", true)?;

        let version: i32 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        match version {
            0 => {
                connection.execute_batch(
                    "CREATE TABLE IF NOT EXISTS sessions (
                        id          TEXT PRIMARY KEY,
                        title       TEXT NOT NULL,
                        backend     TEXT NOT NULL,
                        model       TEXT NOT NULL,
                        started_at  INTEGER NOT NULL,
                        turns       INTEGER NOT NULL,
                        record      TEXT,
                        view        TEXT NOT NULL,
                        saved_at    INTEGER NOT NULL
                    );",
                )?;
                connection.pragma_update(None, "user_version", SCHEMA)?;
            }
            SCHEMA => {}
            // Refused rather than guessed at. A newer file read by an older
            // binary is the one case where carrying on writes a row nobody can
            // read back, and "the store is a cache" is not licence to corrupt
            // it.
            other => anyhow::bail!(
                "{other} is not a schema this build knows (it writes {SCHEMA}); \
                 the store was written by a newer luu"
            ),
        }
        Ok(Self { connection })
    }

    /// Writes the fold, replacing whatever was there.
    ///
    /// A whole-row replace rather than a patch, because the thing being stored
    /// is a cache of a fold and a partially-updated cache of a fold is not a
    /// fold of anything.
    pub fn save(&self, view: &SessionView) -> Result<()> {
        let summary = view.summary();
        self.connection.execute(
            "INSERT INTO sessions
                 (id, title, backend, model, started_at, turns, record, view, saved_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(id) DO UPDATE SET
                 title = excluded.title,
                 backend = excluded.backend,
                 model = excluded.model,
                 started_at = excluded.started_at,
                 turns = excluded.turns,
                 record = excluded.record,
                 view = excluded.view,
                 saved_at = excluded.saved_at",
            rusqlite::params![
                summary.id,
                summary.title,
                summary.backend,
                summary.model,
                summary.started_at as i64,
                summary.turns as i64,
                summary.record,
                serde_json::to_string(view)?,
                crate::session::now_ms() as i64,
            ],
        )?;
        Ok(())
    }

    /// The fold back, out of the blob.
    ///
    /// Deliberately not assembled from the columns beside it: those are a
    /// listing convenience, and a `load` that read them would make a column
    /// that had drifted from the view invisible.
    pub fn load(&self, id: &str) -> Result<Option<SessionView>> {
        let mut statement = self
            .connection
            .prepare("SELECT view FROM sessions WHERE id = ?1")?;
        let mut rows = statement.query([id])?;
        let Some(row) = rows.next()? else {
            return Ok(None);
        };
        let json: String = row.get(0)?;
        Ok(Some(serde_json::from_str(&json).with_context(|| {
            format!("the stored fold for session {id}")
        })?))
    }

    /// Resumes a stored session into an active context — the inverse fold.
    pub fn resume(
        &self,
        id: &str,
        system: impl Into<String>,
        tools: impl Into<String>,
        map: impl Into<String>,
        counter: &dyn agent_core::context::TokenCounter,
    ) -> Result<Option<agent_core::context::Context>> {
        let Some(view) = self.load(id)? else {
            return Ok(None);
        };
        Ok(Some(agent_core::context::Context::from_view(
            &view, system, tools, map, counter,
        )))
    }

    /// What `GET /api/sessions` lists, newest first.
    pub fn list(&self) -> Result<Vec<SessionSummary>> {
        let mut statement = self.connection.prepare(
            "SELECT id, title, backend, model, started_at, turns, record
             FROM sessions ORDER BY started_at DESC, id DESC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(SessionSummary {
                id: row.get(0)?,
                title: row.get(1)?,
                backend: row.get(2)?,
                model: row.get(3)?,
                started_at: row.get::<_, i64>(4)? as u64,
                turns: row.get::<_, i64>(5)? as usize,
                record: row.get(6)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn delete(&self, id: &str) -> Result<bool> {
        let removed = self
            .connection
            .execute("DELETE FROM sessions WHERE id = ?1", [id])?;
        Ok(removed > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::api::SessionView;

    fn view(id: &str) -> SessionView {
        let mut view = SessionView::new(id, "mock", "mock");
        view.started_at = 1_700_000_000_000;
        view
    }

    #[test]
    fn a_session_saved_comes_back_as_the_same_fold() {
        let store = SessionStore::in_memory().expect("a store");
        let view = view("one");
        store.save(&view).expect("saving");

        let loaded = store.load("one").expect("loading").expect("the session");
        assert_eq!(
            serde_json::to_value(&loaded).unwrap(),
            serde_json::to_value(&view).unwrap(),
        );
    }

    #[test]
    fn saving_twice_replaces_rather_than_duplicates() {
        let store = SessionStore::in_memory().expect("a store");
        store.save(&view("one")).expect("saving");
        let mut later = view("one");
        later.title = "a name someone gave it".into();
        store.save(&later).expect("saving again");

        let listed = store.list().expect("listing");
        assert_eq!(listed.len(), 1, "one session, saved at two checkpoints");
        assert_eq!(listed[0].title, "a name someone gave it");
    }

    #[test]
    fn a_session_that_was_never_saved_is_none_rather_than_an_error() {
        let store = SessionStore::in_memory().expect("a store");
        assert!(store.load("nothing").expect("loading").is_none());
        assert!(!store.delete("nothing").expect("deleting"));
    }

    #[test]
    fn the_listing_columns_are_the_folds_own_summary() {
        let store = SessionStore::in_memory().expect("a store");
        let mut view = view("one");
        view.record = Some("runs/one.jsonl".into());
        store.save(&view).expect("saving");

        let listed = store.list().expect("listing");
        assert_eq!(
            serde_json::to_value(&listed[0]).unwrap(),
            serde_json::to_value(view.summary()).unwrap(),
            "a column that stopped matching the fold is a second truth",
        );
    }
}
