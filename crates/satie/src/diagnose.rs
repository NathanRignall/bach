//! What looks wrong with a task, in words an agent (or a person) can act on.
use crate::{
    probe::{self, Listener},
    process::now_ms,
    Task, TaskStatus,
};
use std::{collections::HashMap, path::Path};

/// A task must be this old before a port that isn't listening yet counts as a problem.
pub(crate) const SETTLE_MS: i64 = 10_000;

/// Who holds a port, in words an agent (or a person) can act on.
pub(crate) fn holder_text(
    holder: &Listener,
    tasks_by_session: &HashMap<u32, (String, String)>,
) -> String {
    if let Some((id, name)) = holder.sid.and_then(|s| tasks_by_session.get(&s)) {
        return format!("Satie task {id} \"{name}\"");
    }
    let Some(pid) = holder.pid else {
        return holder.command.clone();
    };
    let mut w = format!("pid {pid} (`{}`", holder.command);
    if let Some(cwd) = &holder.cwd {
        w += &format!(", in {cwd}");
    }
    if let Some(up) = holder.up_secs {
        w += &format!(", up {}", probe::age(up));
    }
    w + "), not a Satie task"
}

/// Says what looks wrong with a task: ports it needs that something else already holds, and
/// expected ports that never came up. Returns `(problems, missing expected ports)`.
pub(crate) fn diagnose(
    task: &Task,
    own: &[u16],
    listening: &[Listener],
    tasks_by_session: &HashMap<u32, (String, String)>,
    settle_ms: i64,
) -> (Vec<String>, Vec<u16>) {
    let foreign = |port: u16| {
        listening
            .iter()
            .find(|l| l.port == port && l.sid != Some(task.pid))
    };
    let mut problems = vec![];
    let mut explained: Vec<u16> = vec![];

    // Ports the log complains are taken, and who really holds them right now.
    let log = probe::log_for_scanning(Path::new(&task.log_path));
    for port in probe::ports_named_in_conflicts(&log) {
        if let Some(holder) = foreign(port) {
            problems.push(format!(
                "Port {port} is already in use by {}.",
                holder_text(holder, tasks_by_session)
            ));
            explained.push(port);
        }
    }

    // Ports it should be serving that it isn't: taken by something else, or just not up. A task
    // that is still starting gets a moment; one that already failed is judged straight away.
    let unmet: Vec<u16> = match task.status {
        TaskStatus::Running if now_ms() - task.started_at >= settle_ms => task
            .expected_ports
            .iter()
            .copied()
            .filter(|p| !own.contains(p))
            .collect(),
        TaskStatus::Failed => task
            .expected_ports
            .iter()
            .copied()
            .filter(|p| !own.contains(p))
            .collect(),
        _ => vec![],
    };
    for p in &unmet {
        if explained.contains(p) {
            continue;
        }
        problems.push(match foreign(*p) {
            Some(holder) => format!(
                "Port {p} is already in use by {}.",
                holder_text(holder, tasks_by_session)
            ),
            None => format!("Expected port {p} is not listening; check the task's logs."),
        });
    }
    // Only a running task can still bring its ports up.
    let missing = if task.status == TaskStatus::Running {
        unmet
    } else {
        vec![]
    };
    (problems, missing)
}

/// Dev stacks open dozens of sockets: inspector ports, random high ports for internal IPC.
/// Show the ones a person would recognise; only if a task listens on nothing else, show those.
pub fn presentable_ports(all: Vec<u16>) -> Vec<u16> {
    // Above this is Linux's ephemeral range, where programs get *arbitrary* ports.
    const EPHEMERAL: u16 = 32768;
    let named: Vec<u16> = all.iter().copied().filter(|p| *p < EPHEMERAL).collect();
    if named.is_empty() {
        all
    } else {
        named
    }
}
