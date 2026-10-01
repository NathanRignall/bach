//! Words for carrying a session's work into another session: the draft of a handoff message, and
//! a transcript to start a fork from when its agent can't continue the conversation itself.
use bach_protocol::{AgentEvent, AgentKind, Entry, LogEntry, QueuedMessage};

/// A run's last message to the user: what the agent said when it finished (text from sub-agents
/// doesn't count).
fn last_reply<'a>(entries: &'a [LogEntry]) -> Option<&'a str> {
    entries.iter().rev().find_map(|e| match &e.entry {
        Entry::Agent { event: AgentEvent::Text { text, parent: None }, .. } if !text.trim().is_empty() => Some(text.trim()),
        _ => None,
    })
}

/// The messages the user sent, each with what followed it up to the next one.
fn turns(entries: &[LogEntry]) -> Vec<(&str, &[LogEntry])> {
    let starts: Vec<usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| matches!(e.entry, Entry::User { .. }))
        .map(|(i, _)| i)
        .collect();
    starts
        .iter()
        .enumerate()
        .map(|(n, &i)| {
            let end = starts.get(n + 1).copied().unwrap_or(entries.len());
            let Entry::User { text, .. } = &entries[i].entry else { unreachable!() };
            (text.as_str(), &entries[i + 1..end])
        })
        .collect()
}

/// `text` cut to about `max` characters, at a word, on one line.
fn brief(text: &str, max: usize) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        return flat;
    }
    let cut: String = flat.chars().take(max).collect();
    let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(head, _)| head);
    format!("{cut}…")
}

/// How many of the session's earlier exchanges a handoff message lists.
const LISTED_TURNS: usize = 10;

/// A draft message for another agent to carry on the session's work: its goal, what was done,
/// where it stands, what is waiting, and the files touched. Written from the transcript and the
/// worktree's changes, for the user to edit before it is sent.
pub fn summary(title: &str, from: AgentKind, entries: &[LogEntry], queued: &[QueuedMessage], files: &[String]) -> String {
    let turns = turns(entries);
    let mut out = format!(
        "You are taking over a task from another coding agent ({}), which worked on it in this same folder. \
         Its changes are already here. Session: \"{title}\".\n",
        from.display_name()
    );
    if let Some((goal, _)) = turns.first() {
        out += &format!("\n## Goal\n{}\n", goal.trim());
    }
    out += "\n## What was done\n";
    if turns.is_empty() {
        out += "Nothing yet.\n";
    }
    if turns.len() > LISTED_TURNS {
        out += &format!("({} earlier messages left out.)\n", turns.len() - LISTED_TURNS);
    }
    for (asked, after) in turns.iter().skip(turns.len().saturating_sub(LISTED_TURNS)) {
        let did = last_reply(after).map(|r| brief(r, 280));
        let stopped = after.iter().any(|e| matches!(e.entry, Entry::Agent { event: AgentEvent::Cancelled, .. }));
        out += &format!("- Asked: {}\n", brief(asked, 160));
        match (did, stopped) {
            (Some(d), _) => out += &format!("  Result: {d}\n"),
            (None, true) => out += "  Stopped before it finished.\n",
            (None, false) => out += "  No reply recorded.\n",
        }
    }
    if let Some(reply) = last_reply(entries) {
        out += &format!("\n## Where it stands\nThe last thing the agent said:\n\n{}\n", brief(reply, 1500));
    }
    out += "\n## Left to do\n";
    if queued.is_empty() {
        out += "Carry on from where it stands; the files below show what has been worked on.\n";
    } else {
        out += "Messages that were waiting and never sent:\n";
        for q in queued {
            out += &format!("- {}\n", brief(&q.text, 200));
        }
    }
    if !files.is_empty() {
        out += "\n## Files touched\n";
        for f in files.iter().take(60) {
            out += &format!("- {f}\n");
        }
        if files.len() > 60 {
            out += &format!("- … and {} more\n", files.len() - 60);
        }
    }
    out
}

/// How much of a transcript a seeded fork carries (the most recent part).
const SEED_CHARS: usize = 40_000;

/// The conversation so far, as text to put before a fork's first message when its agent can't
/// fork its own conversation: what was said, and which tools were used (not their output).
pub fn transcript_seed(entries: &[LogEntry]) -> String {
    let mut lines: Vec<String> = vec![];
    for e in entries {
        match &e.entry {
            Entry::User { text, .. } => lines.push(format!("User: {}", text.trim())),
            Entry::Agent { event: AgentEvent::Text { text, parent: None }, .. } => lines.push(format!("Assistant: {}", text.trim())),
            Entry::Agent { event: AgentEvent::ToolUse { name, input, parent: None, .. }, .. } => {
                lines.push(format!("[Assistant used {name}: {}]", brief(&input.to_string(), 120)))
            }
            _ => {}
        }
    }
    let mut kept = 0;
    let mut start = lines.len();
    while start > 0 && kept + lines[start - 1].len() <= SEED_CHARS {
        start -= 1;
        kept += lines[start].len();
    }
    let omitted = if start > 0 { "(Earlier messages left out.)\n" } else { "" };
    format!(
        "This conversation continues an earlier one, which is included here for context. Its work is already in this folder.\n\n\
         <earlier conversation>\n{omitted}{}\n</earlier conversation>",
        lines[start..].join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn log(entries: Vec<Entry>) -> Vec<LogEntry> {
        entries.into_iter().enumerate().map(|(i, entry)| LogEntry { seq: i as u64 + 1, at: 0, entry }).collect()
    }
    fn user(t: &str) -> Entry {
        Entry::User { text: t.into(), images: vec![] }
    }
    fn say(t: &str) -> Entry {
        Entry::Agent { run_id: "r".into(), event: AgentEvent::Text { text: t.into(), parent: None } }
    }

    #[test]
    fn the_summary_says_goal_progress_queue_and_files() {
        let entries = log(vec![
            user("Add a login page"),
            say("Created the page."),
            Entry::Agent { run_id: "r".into(), event: AgentEvent::Text { text: "sub-agent chatter".into(), parent: Some("t".into()) } },
            user("Now add tests"),
            say("Added two tests,\nthey pass."),
        ]);
        let queued = [QueuedMessage { id: "q".into(), text: "then docs".into(), images: vec![] }];
        let s = summary("Login", AgentKind::Claude, &entries, &queued, &["src/login.rs".into()]);
        assert!(s.contains("Claude Code") && s.contains("\"Login\""));
        assert!(s.contains("## Goal\nAdd a login page"));
        assert!(s.contains("- Asked: Now add tests\n  Result: Added two tests, they pass."));
        assert!(s.contains("## Where it stands\nThe last thing the agent said:\n\nAdded two tests, they pass."));
        assert!(!s.contains("sub-agent chatter"));
        assert!(s.contains("Messages that were waiting and never sent:\n- then docs"));
        assert!(s.ends_with("## Files touched\n- src/login.rs\n"));
    }

    #[test]
    fn a_short_session_and_an_empty_one() {
        let s = summary("Empty", AgentKind::Codex, &[], &[], &[]);
        assert!(s.contains("Nothing yet.") && !s.contains("## Goal") && !s.contains("Files touched"));
        let stopped = log(vec![user("Do it"), Entry::Agent { run_id: "r".into(), event: AgentEvent::Cancelled }]);
        assert!(summary("T", AgentKind::Claude, &stopped, &[], &[]).contains("Stopped before it finished."));
    }

    #[test]
    fn long_sessions_list_the_recent_exchanges() {
        let mut entries = vec![];
        for i in 0..14 {
            entries.push(user(&format!("message {i}")));
            entries.push(say(&format!("reply {i}")));
        }
        let s = summary("Long", AgentKind::Claude, &log(entries), &[], &[]);
        assert!(s.contains("(4 earlier messages left out.)") && !s.contains("Asked: message 3\n") && s.contains("Asked: message 4\n"));
        assert!(s.contains("## Goal\nmessage 0"));
    }

    #[test]
    fn the_seed_is_the_conversation_with_tools_named_and_old_parts_dropped() {
        let entries = log(vec![
            user("hi"),
            say("hello"),
            Entry::Agent { run_id: "r".into(), event: AgentEvent::ToolUse { id: "t".into(), name: "Edit".into(), input: json!({"file_path": "a.rs"}), parent: None } },
            Entry::Agent { run_id: "r".into(), event: AgentEvent::Thinking { text: "hmm".into() } },
        ]);
        let s = transcript_seed(&entries);
        assert!(s.contains("User: hi\nAssistant: hello\n[Assistant used Edit: {\"file_path\":\"a.rs\"}]\n</earlier conversation>"));
        assert!(!s.contains("hmm") && !s.contains("left out"));
        let big = log((0..3).map(|i| user(&format!("{i}{}", "x".repeat(SEED_CHARS / 2)))).collect());
        let s = transcript_seed(&big);
        assert!(s.contains("(Earlier messages left out.)") && !s.contains("User: 0x") && s.contains("User: 2x"));
    }
}
