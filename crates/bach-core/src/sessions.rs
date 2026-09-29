//! Sessions as the backend keeps them: stored, recorded as their runs go, and announced to
//! clients as [`SessionEvent`]s.
use crate::{
    runs::{Emit, RunEvent},
    store::Store,
};
use bach_protocol::{
    AgentEvent, ApiError, ContextUsage, Entry, LogEntry, PlanUsage, ServerEvent, Session,
    SessionEvent,
};
use std::sync::Arc;
use tokio::sync::broadcast;

/// Called with a session whose run (the given id) just finished cleanly.
pub type OnFinish = Arc<dyn Fn(&mut Session, &str) + Send + Sync>;

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

    /// The account's usage limits as last reported by any run.
    pub fn plan_usage(&self) -> Result<Option<PlanUsage>, ApiError> {
        Ok(self.store.plan_usage()?)
    }

    /// Where a run of session `id` sends its events. `on_finish` changes the session in the same
    /// step that marks a cleanly finished run over, so nobody sees what's in between.
    pub fn recorder(&self, id: &str, on_finish: Option<OnFinish>) -> Emit {
        let (sessions, id) = (self.clone(), id.to_string());
        Arc::new(move |ev| sessions.record(&id, ev, on_finish.clone()))
    }

    /// Records one event of a session's run, and what it means for the session.
    fn record(&self, id: &str, RunEvent { run_id, event }: RunEvent, on_finish: Option<OnFinish>) {
        // Usage readings describe the session (or the account), not what happened in it.
        match event {
            AgentEvent::Limits { usage } => {
                if let Err(e) = self.store.set_plan_usage(&usage) {
                    eprintln!("couldn't save usage limits: {e}");
                }
                let _ = self.events.send(ServerEvent::Usage(usage));
                return;
            }
            AgentEvent::Context { used } => {
                let _ = self.update(id, |s| {
                    let window = s.context.as_ref().and_then(|c| c.window);
                    s.context = Some(ContextUsage { used, window });
                    Ok(())
                });
                return;
            }
            AgentEvent::ContextWindows { windows } => {
                let _ = self.update(id, |s| {
                    // The session's own model; failing that, the largest (sub-agents often use
                    // smaller models).
                    let window = s
                        .model
                        .as_ref()
                        .and_then(|m| windows.get(m))
                        .or_else(|| windows.values().max())
                        .copied();
                    let used = s.context.as_ref().map_or(0, |c| c.used);
                    s.context = Some(ContextUsage { used, window });
                    Ok(())
                });
                return;
            }
            AgentEvent::TextDelta { id: message, text } => {
                self.send(SessionEvent::TextDelta {
                    session_id: id.into(),
                    run_id,
                    id: message,
                    text,
                });
                return;
            }
            _ => {}
        }
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
            AgentEvent::Done { .. } | AgentEvent::Cancelled => {
                let clean = matches!(event, AgentEvent::Done { is_error: false, .. });
                Some(Box::new(move |s| {
                    if s.run_id.as_deref() == Some(&run_id) {
                        s.run_id = None;
                        s.open_approvals.clear();
                        if let (true, Some(f)) = (clean, on_finish) {
                            f(s, &run_id);
                        }
                    }
                }))
            }
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
