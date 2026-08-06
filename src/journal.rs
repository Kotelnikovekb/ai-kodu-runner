use crate::state::State;
use anyhow::{Context, Result};
use rusqlite::{Connection, params};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};
#[derive(Clone)]
pub struct Journal {
    db: Arc<Mutex<Connection>>,
}
impl Journal {
    pub fn open(path: &Path) -> Result<Self> {
        let c = Connection::open(path).context("open journal")?;
        c.execute_batch("CREATE TABLE IF NOT EXISTS jobs (job_id TEXT NOT NULL, attempt INTEGER NOT NULL, state TEXT NOT NULL, updated_at TEXT NOT NULL, PRIMARY KEY(job_id,attempt)); CREATE TABLE IF NOT EXISTS events (id INTEGER PRIMARY KEY, job_id TEXT, attempt INTEGER, state TEXT, at TEXT NOT NULL);").context("initialize journal")?;
        Ok(Self {
            db: Arc::new(Mutex::new(c)),
        })
    }
    pub fn transition(&self, id: &str, attempt: u32, state: State) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let c = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("journal lock poisoned"))?;
        c.execute("INSERT INTO jobs(job_id,attempt,state,updated_at) VALUES(?1,?2,?3,?4) ON CONFLICT(job_id,attempt) DO UPDATE SET state=excluded.state,updated_at=excluded.updated_at",params![id,attempt,state.as_str(),now])?;
        c.execute(
            "INSERT INTO events(job_id,attempt,state,at) VALUES(?1,?2,?3,?4)",
            params![id, attempt, state.as_str(), now],
        )?;
        Ok(())
    }
    pub fn next_attempt(&self, id: &str) -> Result<u32> {
        let c = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("journal lock poisoned"))?;
        let next: u32 = c.query_row(
            "SELECT COALESCE(MAX(attempt), 0) + 1 FROM jobs WHERE job_id = ?1",
            params![id],
            |row| row.get(0),
        )?;
        Ok(next)
    }
    pub fn unfinished(&self) -> Result<Vec<(String, u32)>> {
        let c = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("journal lock poisoned"))?;
        let mut s=c.prepare("SELECT job_id,attempt FROM jobs WHERE state NOT IN ('destroyed','completed','failed','cancelled','timed_out')")?;
        Ok(s.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }
}
