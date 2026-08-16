// Copyright 2026 Kotelnikovekb
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://apache.org
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
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

#[derive(Debug, Clone)]
pub struct PendingCompletion {
    pub job_id: String,
    pub attempt: u32,
    pub lease_id: String,
    pub idempotency_key: String,
    pub payload: String,
    pub attempts: u32,
}
impl Journal {
    pub fn open(path: &Path) -> Result<Self> {
        let c = Connection::open(path).context("open journal")?;
        c.execute_batch("CREATE TABLE IF NOT EXISTS jobs (job_id TEXT NOT NULL, attempt INTEGER NOT NULL, state TEXT NOT NULL, updated_at TEXT NOT NULL, PRIMARY KEY(job_id,attempt)); CREATE TABLE IF NOT EXISTS events (id INTEGER PRIMARY KEY, job_id TEXT, attempt INTEGER, state TEXT, at TEXT NOT NULL); CREATE TABLE IF NOT EXISTS completion_outbox (job_id TEXT NOT NULL, attempt INTEGER NOT NULL, lease_id TEXT NOT NULL, idempotency_key TEXT NOT NULL, payload TEXT NOT NULL, attempts INTEGER NOT NULL DEFAULT 0, delivered_at TEXT, updated_at TEXT NOT NULL, PRIMARY KEY(job_id, attempt));").context("initialize journal")?;
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

    pub fn enqueue_completion(
        &self,
        job_id: &str,
        attempt: u32,
        lease_id: &str,
        idempotency_key: &str,
        payload: &str,
    ) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let c = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("journal lock poisoned"))?;
        c.execute(
            "INSERT INTO completion_outbox(job_id,attempt,lease_id,idempotency_key,payload,updated_at) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(job_id,attempt) DO NOTHING",
            params![job_id, attempt, lease_id, idempotency_key, payload, now],
        )?;
        Ok(())
    }

    pub fn pending_completions(&self) -> Result<Vec<PendingCompletion>> {
        let c = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("journal lock poisoned"))?;
        let mut statement = c.prepare(
            "SELECT job_id,attempt,lease_id,idempotency_key,payload,attempts FROM completion_outbox WHERE delivered_at IS NULL ORDER BY updated_at",
        )?;
        Ok(statement
            .query_map([], |row| {
                Ok(PendingCompletion {
                    job_id: row.get(0)?,
                    attempt: row.get(1)?,
                    lease_id: row.get(2)?,
                    idempotency_key: row.get(3)?,
                    payload: row.get(4)?,
                    attempts: row.get(5)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn mark_completion_delivered(&self, job_id: &str, attempt: u32) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let c = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("journal lock poisoned"))?;
        c.execute(
            "UPDATE completion_outbox SET delivered_at=?3,updated_at=?3 WHERE job_id=?1 AND attempt=?2",
            params![job_id, attempt, now],
        )?;
        Ok(())
    }

    pub fn record_completion_attempt(&self, job_id: &str, attempt: u32) -> Result<()> {
        let now = chrono::Utc::now().to_rfc3339();
        let c = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("journal lock poisoned"))?;
        c.execute(
            "UPDATE completion_outbox SET attempts=attempts+1,updated_at=?3 WHERE job_id=?1 AND attempt=?2",
            params![job_id, attempt, now],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::Journal;

    #[test]
    fn completion_outbox_survives_retry_and_marks_delivery() {
        let directory = tempfile::tempdir().unwrap();
        let journal = Journal::open(&directory.path().join("runner.db")).unwrap();

        journal
            .enqueue_completion("job-1", 2, "lease-1", "key-1", "{\"job_id\":\"job-1\"}")
            .unwrap();
        journal.record_completion_attempt("job-1", 2).unwrap();

        let pending = journal.pending_completions().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].job_id, "job-1");
        assert_eq!(pending[0].attempt, 2);
        assert_eq!(pending[0].attempts, 1);
        assert_eq!(pending[0].idempotency_key, "key-1");

        journal.mark_completion_delivered("job-1", 2).unwrap();
        assert!(journal.pending_completions().unwrap().is_empty());
    }

    #[test]
    fn enqueue_is_idempotent_for_same_attempt() {
        let directory = tempfile::tempdir().unwrap();
        let journal = Journal::open(&directory.path().join("runner.db")).unwrap();

        journal
            .enqueue_completion("job-1", 1, "lease-1", "key-1", "payload-1")
            .unwrap();
        journal.record_completion_attempt("job-1", 1).unwrap();
        journal
            .enqueue_completion("job-1", 1, "lease-1", "key-1", "payload-2")
            .unwrap();

        let pending = journal.pending_completions().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].payload, "payload-1");
        assert_eq!(pending[0].attempts, 1);
    }
}
