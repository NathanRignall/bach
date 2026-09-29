//! Runs recorded output of the real CLIs through the adapters.
use bach_core::adapters::{AgentEvent, AgentKind};

fn events(agent: AgentKind, fixture: &str) -> Vec<AgentEvent> {
    fixture.lines().flat_map(|l| agent.parse_line(l)).collect()
}

#[test]
fn claude_real_bash_run() {
    let ev = events(
        AgentKind::Claude,
        include_str!("fixtures/claude_bash.jsonl"),
    );
    assert!(matches!(&ev[0], AgentEvent::Session { id } if id.len() == 36));
    assert!(ev
        .iter()
        .any(|e| matches!(e, AgentEvent::ToolUse { name, .. } if name == "Bash")));
    assert!(ev.iter().any(|e| matches!(e, AgentEvent::ToolResult { output, is_error: false, .. } if output == "hello-bach")));
    assert!(ev
        .iter()
        .any(|e| matches!(e, AgentEvent::Text { text } if text == "done")));
    assert!(ev.iter().any(|e| matches!(
        e,
        AgentEvent::Done {
            cost_usd: Some(_),
            is_error: false
        }
    )));
    // Redacted thinking blocks arrive with empty text; the UI shouldn't get an empty block.
    assert!(!ev
        .iter()
        .any(|e| matches!(e, AgentEvent::Thinking { text } if text.is_empty())));
    assert!(!ev.iter().any(|e| matches!(e, AgentEvent::Raw { .. })));
}
