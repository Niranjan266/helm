//! `term.open` / `write` / `resize` / `close` (PROTOCOL §13): ConPTY shells owned by one
//! control session. Output goes out as `term.output{term_id,data_b64}`, the end as
//! `term.exit{term_id,code}`. A session's shells are killed when it disconnects.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::platform::{Shell, TermSession, blocking};
use crate::router::RpcError;
use crate::server::AppState;
use crate::session::Session;
use crate::util;

use super::lock;

/// Shells one session may have open at once.
pub const MAX_PER_SESSION: usize = 8;
/// Largest `term.write` payload (decoded).
pub const MAX_WRITE: usize = 64 * 1024;
/// Largest `cols` / `rows` accepted from the phone.
pub const MAX_SIZE: u16 = 1000;

struct Entry {
    session_id: u64,
    term: Arc<dyn TermSession>,
}

#[derive(Default)]
pub struct Terms {
    map: Arc<Mutex<HashMap<String, Entry>>>,
}

/// `"powershell"` / `"cmd"` → shell.
pub fn parse_shell(name: &str) -> Result<Shell, RpcError> {
    match name {
        "powershell" => Ok(Shell::PowerShell),
        "cmd" => Ok(Shell::Cmd),
        other => Err(RpcError::bad_params(format!(
            "shell must be \"powershell\" or \"cmd\", not {other:?}"
        ))),
    }
}

pub fn check_size(cols: u64, rows: u64) -> Result<(u16, u16), RpcError> {
    let max = u64::from(MAX_SIZE);
    if !(1..=max).contains(&cols) || !(1..=max).contains(&rows) {
        return Err(RpcError::bad_params(format!(
            "cols and rows must be 1..={MAX_SIZE}"
        )));
    }
    Ok((cols as u16, rows as u16))
}

impl Terms {
    pub fn count_for(&self, session_id: u64) -> usize {
        lock(&self.map)
            .values()
            .filter(|e| e.session_id == session_id)
            .count()
    }

    fn get(&self, term_id: &str, session_id: u64) -> Result<Arc<dyn TermSession>, RpcError> {
        lock(&self.map)
            .get(term_id)
            .filter(|e| e.session_id == session_id)
            .map(|e| e.term.clone())
            .ok_or_else(|| RpcError::bad_params(format!("no open terminal {term_id:?}")))
    }

    /// Start a shell for `session`; returns its `term_id`.
    pub async fn open(
        &self,
        state: &Arc<AppState>,
        session: &Arc<Session>,
        shell: Shell,
        cols: u16,
        rows: u16,
    ) -> Result<String, RpcError> {
        if self.count_for(session.id) >= MAX_PER_SESSION {
            return Err(RpcError::bad_params(format!(
                "at most {MAX_PER_SESSION} terminals per connection"
            )));
        }
        let term_id = hex::encode(util::random_bytes(8));
        let exited = Arc::new(AtomicBool::new(false));
        let out_session = Arc::downgrade(session);
        let out_id = term_id.clone();
        let on_output = Box::new(move |bytes: &[u8]| {
            if let Some(s) = out_session.upgrade() {
                s.send_event(
                    "term.output",
                    json!({"term_id": out_id, "data_b64": util::b64_encode(bytes)}),
                );
            }
        });
        let exit_session = Arc::downgrade(session);
        let exit_id = term_id.clone();
        let exit_map = self.map.clone();
        let exit_flag = exited.clone();
        let on_exit = Box::new(move |code: u32| {
            exit_flag.store(true, Ordering::SeqCst);
            let entry = lock(&exit_map).remove(&exit_id);
            if let Some(s) = exit_session.upgrade() {
                s.send_event("term.exit", json!({"term_id": exit_id, "code": code}));
            }
            // Dropping the session from its own exit callback is fine (it does not join).
            drop(entry);
        });
        let p = state.platform.clone();
        let term: Arc<dyn TermSession> =
            Arc::from(blocking(move || p.open_terminal(shell, cols, rows, on_output, on_exit)).await?);
        lock(&self.map).insert(
            term_id.clone(),
            Entry {
                session_id: session.id,
                term,
            },
        );
        if exited.load(Ordering::SeqCst) {
            // Ended before it was registered.
            lock(&self.map).remove(&term_id);
        }
        Ok(term_id)
    }

    pub async fn write(&self, term_id: &str, session_id: u64, bytes: Vec<u8>) -> Result<(), RpcError> {
        let term = self.get(term_id, session_id)?;
        blocking(move || term.write(&bytes)).await
    }

    pub async fn resize(
        &self,
        term_id: &str,
        session_id: u64,
        cols: u16,
        rows: u16,
    ) -> Result<(), RpcError> {
        let term = self.get(term_id, session_id)?;
        blocking(move || term.resize(cols, rows)).await
    }

    pub async fn close(&self, term_id: &str, session_id: u64) -> Result<(), RpcError> {
        self.get(term_id, session_id)?;
        let entry = lock(&self.map).remove(term_id);
        if let Some(e) = entry {
            kill_all(vec![e.term]).await;
        }
        Ok(())
    }

    /// The session ended: kill its shells.
    pub async fn close_session(&self, session_id: u64) {
        let terms: Vec<Arc<dyn TermSession>> = {
            let mut map = lock(&self.map);
            let ids: Vec<String> = map
                .iter()
                .filter(|(_, e)| e.session_id == session_id)
                .map(|(id, _)| id.clone())
                .collect();
            ids.iter()
                .filter_map(|id| map.remove(id))
                .map(|e| e.term)
                .collect()
        };
        if !terms.is_empty() {
            kill_all(terms).await;
        }
    }
}

async fn kill_all(terms: Vec<Arc<dyn TermSession>>) {
    let _ = tokio::task::spawn_blocking(move || {
        for t in terms {
            let _ = t.kill();
            drop(t);
        }
    })
    .await;
}
