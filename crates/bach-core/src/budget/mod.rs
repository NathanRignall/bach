//! The account's spend against an LLM API budget, for agents that run through a proxy that keeps
//! one. Where it's read from is a [`Source`]; none is built in, so a build without one never
//! looks it up and clients show no budget. A build that has one adds it to [`sources`].
use crate::store::now_ms;
use bach_protocol::{BudgetUsage, ServerEvent};
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::broadcast;

/// Looked up this often while the server runs, and at most this often after runs finish.
const EVERY: Duration = Duration::from_secs(10 * 60);
const AT_MOST_EVERY: Duration = Duration::from_secs(60);

/// Somewhere the budget can be read.
pub trait Source: Send + Sync {
    /// What it's called in the server's log.
    fn name(&self) -> &str;
    /// The budget now, with `observed_at` left for the caller.
    fn fetch(&self) -> Pin<Box<dyn Future<Output = Result<BudgetUsage, String>> + Send + '_>>;
}

/// The sources this build knows, in order of preference; the first that applies is used.
fn sources() -> Vec<Box<dyn Source>> {
    vec![]
}

#[derive(Clone)]
pub struct Budget(Option<Arc<Inner>>);

struct Inner {
    source: Box<dyn Source>,
    events: broadcast::Sender<ServerEvent>,
    last: Mutex<Option<BudgetUsage>>,
    asked: Mutex<Option<Instant>>,
}

impl Budget {
    /// Follows the budget from the first of [`sources`], if there is one.
    pub fn new(events: broadcast::Sender<ServerEvent>) -> Budget {
        Self::from(sources().into_iter().next(), events)
    }

    fn from(source: Option<Box<dyn Source>>, events: broadcast::Sender<ServerEvent>) -> Budget {
        let Some(source) = source else { return Budget(None) };
        let budget = Budget(Some(Arc::new(Inner {
            source,
            events,
            last: Mutex::default(),
            asked: Mutex::default(),
        })));
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            let b = budget.clone();
            rt.spawn(async move {
                loop {
                    b.look_up().await;
                    tokio::time::sleep(EVERY).await;
                }
            });
        }
        budget
    }

    /// As last looked up.
    pub fn get(&self) -> Option<BudgetUsage> {
        self.0.as_ref()?.last.lock().unwrap().clone()
    }

    /// Looks it up again soon, unless that was just done: a run has spent some of it.
    pub fn refresh(&self) {
        if self.0.is_none() {
            return;
        }
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            let b = self.clone();
            rt.spawn(async move { b.look_up().await });
        }
    }

    async fn look_up(&self) {
        let Some(inner) = &self.0 else { return };
        {
            let mut asked = inner.asked.lock().unwrap();
            if asked.is_some_and(|at| at.elapsed() < AT_MOST_EVERY) {
                return;
            }
            *asked = Some(Instant::now());
        }
        let usage = match inner.source.fetch().await {
            Ok(usage) => BudgetUsage { observed_at: now_ms(), ..usage },
            Err(e) => return eprintln!("couldn't look up the budget ({}): {e}", inner.source.name()),
        };
        let mut last = inner.last.lock().unwrap();
        if last.as_ref().map(|l| (l.spend, l.max_budget, l.resets_at))
            != Some((usage.spend, usage.max_budget, usage.resets_at))
        {
            let _ = inner.events.send(ServerEvent::Budget(usage.clone()));
        }
        *last = Some(usage);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed(f64);

    impl Source for Fixed {
        fn name(&self) -> &str {
            "fixed"
        }
        fn fetch(&self) -> Pin<Box<dyn Future<Output = Result<BudgetUsage, String>> + Send + '_>> {
            let usage = BudgetUsage { spend: self.0, max_budget: Some(100.0), resets_at: None, observed_at: 0 };
            Box::pin(async move { Ok(usage) })
        }
    }

    #[tokio::test]
    async fn announces_a_budget_once_until_it_changes() {
        let (events, mut rx) = broadcast::channel(8);
        let budget = Budget::from(Some(Box::new(Fixed(12.5))), events);
        for _ in 0..50 {
            if budget.get().is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let got = budget.get().expect("looked up at start");
        assert_eq!((got.spend, got.max_budget), (12.5, Some(100.0)));
        assert!(got.observed_at > 0);
        assert!(matches!(rx.try_recv(), Ok(ServerEvent::Budget(b)) if b.spend == 12.5));
        // Asked again straight away: too soon, so nothing new.
        budget.look_up().await;
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn without_a_source_there_is_no_budget() {
        let (events, _) = broadcast::channel(8);
        let budget = Budget::from(None, events);
        budget.refresh();
        assert!(budget.get().is_none());
    }
}
