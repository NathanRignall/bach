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
    assert!(matches!(&ev[0], AgentEvent::Session { id, .. } if id.len() == 36));
    assert!(ev
        .iter()
        .any(|e| matches!(e, AgentEvent::ToolUse { name, .. } if name == "Bash")));
    assert!(ev.iter().any(|e| matches!(e, AgentEvent::ToolResult { output, is_error: false, .. } if output == "hello-bach")));
    assert!(ev
        .iter()
        .any(|e| matches!(e, AgentEvent::Text { text, .. } if text == "done")));
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

#[test]
fn claude_real_subagent_run() {
    let ev = events(
        AgentKind::Claude,
        include_str!("fixtures/claude_subagent.jsonl"),
    );

    // The model comes from the init event.
    assert!(
        matches!(&ev[0], AgentEvent::Session { model: Some(m), .. } if m.starts_with("claude-"))
    );

    // The main agent spawned a sub-agent through a top-level tool call...
    let agent_id = ev
        .iter()
        .find_map(|e| match e {
            AgentEvent::ToolUse {
                id,
                name,
                parent: None,
                ..
            } if name == "Agent" => Some(id.clone()),
            _ => None,
        })
        .expect("top-level Agent tool call");

    // ...and everything the sub-agent did points back at it instead of being flattened.
    let nested: Vec<_> = ev
        .iter()
        .filter(|e| matches!(e, AgentEvent::ToolUse { parent: Some(p), .. } if *p == agent_id))
        .collect();
    assert!(
        nested.len() >= 2,
        "sub-agent tool calls should be nested: {nested:?}"
    );
    assert!(ev
        .iter()
        .any(|e| matches!(e, AgentEvent::ToolResult { parent: Some(p), .. } if *p == agent_id)));

    // The prompt echoed to the sub-agent is not shown as a message.
    assert!(!ev.iter().any(|e| matches!(e, AgentEvent::Text { text, parent: Some(_) } if text.starts_with("Count how many"))));

    // Lifecycle: started -> progress with counters -> completed with a summary.
    let tasks: Vec<_> = ev
        .iter()
        .filter(|e| matches!(e, AgentEvent::Task { id, .. } if *id == agent_id))
        .collect();
    assert!(
        matches!(tasks.first(), Some(AgentEvent::Task { status: Some(s), title: Some(_), .. }) if s == "running")
    );
    assert!(tasks.iter().any(|t| matches!(t, AgentEvent::Task { tool_uses: Some(n), tokens: Some(_), activity: Some(_), .. } if *n >= 1)));
    assert!(
        matches!(tasks.last(), Some(AgentEvent::Task { status: Some(s), summary: Some(_), .. }) if s == "completed")
    );

    // The hand-back result is readable text, not a JSON dump of content blocks.
    let output = ev.iter().find_map(|e| match e {
        AgentEvent::ToolResult {
            id,
            output,
            parent: None,
            ..
        } if *id == agent_id => Some(output),
        _ => None,
    });
    let output = output.expect("sub-agent hand-back result");
    assert!(output.starts_with("[Subagent hand-back]"), "{output}");
    assert!(
        !output.contains("\"type\""),
        "content blocks should be flattened to text: {output}"
    );
}

#[test]
fn claude_real_approval_request() {
    // The first line is the request the real CLI sent for `tmux ls`; the second is a cancel.
    let ev = events(
        AgentKind::Claude,
        include_str!("fixtures/claude_approval.jsonl"),
    );
    match &ev[0] {
        AgentEvent::Approval {
            request_id,
            tool_use_id,
            tool_name,
            input,
            rules,
            reason,
            suggestions,
            ..
        } => {
            assert_eq!(request_id, "1f0b4978-4c9d-47d8-a464-eb71a79ab709");
            assert_eq!(
                tool_use_id.as_deref(),
                Some("toolu_01Pthzyn5HnJsw47ighS9BsP")
            );
            assert_eq!(tool_name, "Bash");
            assert_eq!(input["command"], "tmux ls");
            assert_eq!(rules, &["Bash(tmux ls *)"]);
            assert_eq!(reason.as_deref(), Some("This command requires approval"));
            assert_eq!(suggestions[0]["destination"], "localSettings");
        }
        other => panic!("expected an approval, got {other:?}"),
    }
    assert!(
        matches!(&ev[1], AgentEvent::ApprovalCancelled { request_id } if request_id.starts_with("1f0b"))
    );
}
