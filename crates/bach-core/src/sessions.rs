//! Sessions as the backend keeps them: stored, recorded as their runs go, and announced to
//! clients as [`SessionEvent`]s.
use crate::{
    runs::{Emit, RunEvent},
    store::Store,
};
use bach_protocol::{AgentEvent, ApiError, Entry, LogEntry, ServerEvent, Session, SessionEvent};
use std::sync::Arc;
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct Sessions {
    store: Store,
    events: broadcast::Sender<ServerEvent>,
}

fn not_found() -> ApiError {
    ApiError::not_found("No such session.")
}

impl Sessions {
    pub fn new(store: Store, events: broadcast::Sender<ServerEvent>) -> Self {
        Self { store, events }
    }

    fn send(&self, ev: SessionEvent) {
        // No subscribers is fine: nobody is watching.
        let _ = self.events.send(ServerEvent::Session(ev));
    }

    pub fn announce(&self, session: &Session) {
        self.send(SessionEvent::Changed {
            session: session.clone(),
        });
    }

    pub fn list(&self) -> Result<Vec<Session>, ApiError> {
        Ok(self.store.list()?)
    }

    pub fn get(&self, id: &str) -> Result<Session, ApiError> {
        self.store.get(id)?.ok_or_else(not_found)
    }

    pub fn entries(&self, id: &str, after: u64) -> Result<Vec<LogEntry>, ApiError> {
        Ok(self.store.entries(id, after)?)
    }

    /// Saves a session without telling anyone yet (see [`announce`](Self::announce)).
    pub fn put_quietly(&self, session: &Session) -> Result<(), ApiError> {
        Ok(self.store.put(session)?)
    }

    pub fn delete(&self, id: &str) -> Result<(), ApiError> {
        self.store.delete(id)?;
        self.send(SessionEvent::Deleted {
            session_id: id.into(),
        });
        Ok(())
    }

    /// Changes a session and announces it. `f` can refuse with an error.
    pub fn update(
        &self,
        id: &str,
        f: impl FnOnce(&mut Session) -> Result<(), ApiError>,
    ) -> Result<Session, ApiError> {
        let s = self.store.update(id, f)?.ok_or_else(not_found)?;
        self.announce(&s);
        Ok(s)
    }

    /// Adds to the transcript and announces the entry.
    pub fn append(&self, id: &str, entry: Entry) -> Result<LogEntry, ApiError> {
        let (log, _) = self.store.append(id, entry)?.ok_or_else(not_found)?;
        self.send(SessionEvent::Entry {
            session_id: id.into(),
            entry: log.clone(),
        });
        Ok(log)
    }

    /// Where a run of session `id` sends its events.
    pub fn recorder(&self, id: &str) -> Emit {
        let (sessions, id) = (self.clone(), id.to_string());
        Arc::new(move |ev| sessions.record(&id, ev))
    }

    /// Records one event of a session's run, and what it means for the session.
    fn record(&self, id: &str, RunEvent { run_id, event }: RunEvent) {
        // A session deleted mid-run still gets its run's last events; they have nowhere to go.
        if self
            .append(
                id,
                Entry::Agent {
                    run_id: run_id.clone(),
                    event: event.clone(),
                },
            )
            .is_err()
        {
            return;
        }
        let effect: Option<Box<dyn FnOnce(&mut Session)>> = match event {
            AgentEvent::Session { id: agent_id, model } => Some(Box::new(move |s| {
                s.agent_session_id = Some(agent_id);
                s.model = model.or(s.model.take());
            })),
            AgentEvent::Approval { request_id, .. } => {
                Some(Box::new(move |s| s.open_approvals.push(request_id)))
            }
            AgentEvent::ApprovalCancelled { request_id } => {
                Some(Box::new(move |s| s.open_approvals.retain(|r| *r != request_id)))
            }
            AgentEvent::Done { .. } | AgentEvent::Cancelled => Some(Box::new(move |s| {
                if s.run_id.as_deref() == Some(&run_id) {
                    s.run_id = None;
                    s.open_approvals.clear();
                }
            })),
            _ => None,
        };
        if let Some(effect) = effect {
            let _ = self.update(id, |s| {
                effect(s);
                Ok(())
            });
        }
    }
}
