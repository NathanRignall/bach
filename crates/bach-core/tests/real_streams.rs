//! Runs recorded output of the real CLIs through the adapters.
use bach_core::adapters::{codex_answer, AgentCli, AgentEvent, AgentKind, Conversation, Turn};
use bach_protocol::Decision;
use serde_json::{json, Value};

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

#[test]
fn claude_reports_context_and_usage_limits() {
    let ev = events(
        AgentKind::Claude,
        include_str!("fixtures/claude_bash.jsonl"),
    );
    // Context after each of the session's own messages: sent (fresh + cached) plus the reply.
    let used: Vec<u64> = ev
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Context { used } => Some(*used),
            _ => None,
        })
        .collect();
    assert_eq!(used.first(), Some(&(2 + 21442 + 12132 + 16)));
    assert_eq!(used.last(), Some(&(2 + 33574 + 2071 + 3)));
    assert!(ev.iter().any(|e| matches!(e, AgentEvent::ContextWindows { windows }
        if windows.get("claude-opus-5-5") == Some(&1_000_000))));

    let usage = ev
        .iter()
        .find_map(|e| match e {
            AgentEvent::Limits { usage } => Some(usage),
            _ => None,
        })
        .expect("a rate_limit_event");
    assert_eq!(usage.status, "allowed");
    assert_eq!(usage.windows["five_hour"].utilization, 0.0);
    assert_eq!(usage.windows["seven_day"].utilization, 0.17);
    assert_eq!(usage.windows["seven_day"].resets_at, Some(1_791_068_400_000), "in ms");

    // A sub-agent's messages don't count towards the session's context.
    let sub = events(
        AgentKind::Claude,
        include_str!("fixtures/claude_subagent.jsonl"),
    );
    let top_level = include_str!("fixtures/claude_subagent.jsonl")
        .lines()
        .filter(|l| l.contains(r#""type":"assistant""#) && l.contains(r#""parent_tool_use_id":null"#))
        .count();
    let contexts = sub.iter().filter(|e| matches!(e, AgentEvent::Context { .. })).count();
    assert_eq!(contexts, top_level);
}

/// Feeds a recorded app-server stream to a new Codex conversation. Returns the events and
/// what Bach wrote back, in order.
fn codex(fixture: &str, allowed: &[String]) -> (Vec<AgentEvent>, Vec<Value>) {
    let (mut c, opening) =
        Conversation::new(AgentKind::Codex, &Turn { prompt: "hi", cwd: Some("/tmp/codex-test/ws"), allowed_tools: allowed, ..Default::default() });
    let mut sent: Vec<Value> = opening.iter().map(|l| serde_json::from_str(l).unwrap()).collect();
    let mut events = vec![];
    for line in fixture.lines() {
        let (ev, replies) = c.on_line(line);
        events.extend(ev);
        sent.extend(replies.iter().map(|l| serde_json::from_str::<Value>(l).unwrap()));
    }
    (events, sent)
}

#[test]
fn codex_real_file_change_asks_first() {
    let (ev, sent) = codex(include_str!("fixtures/codex_file_change.jsonl"), &[]);

    // The handshake: initialize, then a thread that can write to the project, then the turn.
    let methods: Vec<_> = sent.iter().map(|m| m["method"].as_str().unwrap_or("")).collect();
    assert_eq!(methods, ["initialize", "initialized", "thread/start", "turn/start"]);
    assert_eq!(sent[2]["params"]["sandbox"], "workspace-write");
    assert_eq!(sent[2]["params"]["approvalPolicy"], "on-request");
    assert_eq!(sent[2]["params"]["cwd"], "/tmp/codex-test/ws");
    assert_eq!(sent[3]["params"]["threadId"], "01a0ef7b-3246-76a0-80be-4cd98975fbd2");
    assert_eq!(sent[3]["params"]["input"][0]["text"], "hi");

    assert!(matches!(&ev[0], AgentEvent::Session { id, model: Some(m) } if id == "01a0ef7b-3246-76a0-80be-4cd98975fbd2" && m == "gpt-5.6-terra"));
    // The approval shows the change, which only the earlier item carried.
    let approval = ev
        .iter()
        .find_map(|e| match e {
            AgentEvent::Approval { tool_name, input, rules, .. } => Some((tool_name, input, rules)),
            _ => None,
        })
        .unwrap();
    assert_eq!(approval.0, "Edit");
    assert_eq!(approval.1["file_path"], "/tmp/codex-test/ws/hello.txt");
    assert_eq!(approval.1["diff"], "hi\n");
    assert_eq!(approval.2, &["Edit"]);
    assert!(ev.iter().any(|e| matches!(e, AgentEvent::ToolResult { output, is_error: false, .. } if output == "Changed /tmp/codex-test/ws/hello.txt")));
    assert!(ev.iter().any(|e| matches!(e, AgentEvent::ToolUse { name, input, .. } if name == "Shell" && input["command"].as_str().unwrap().starts_with("pwd && rg"))));
    assert!(ev.iter().any(|e| matches!(e, AgentEvent::Text { .. })));
    assert!(ev.iter().any(|e| matches!(e, AgentEvent::Context { used } if *used > 0)));
    assert!(matches!(ev.last(), Some(AgentEvent::Done { is_error: false, .. })));
    assert!(!ev.iter().any(|e| matches!(e, AgentEvent::Raw { .. } | AgentEvent::Error { .. })));
}

#[test]
fn codex_real_command_approval_and_session_rules() {
    let fixture = include_str!("fixtures/codex_command.jsonl");
    let (ev, _) = codex(fixture, &[]);
    let (request_id, input, rules, suggestions) = ev
        .iter()
        .find_map(|e| match e {
            AgentEvent::Approval { request_id, input, rules, suggestions, .. } => {
                Some((request_id, input, rules, suggestions))
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(request_id, "exec-1e8d1ae3-1383-41d0-a3d5-8acb8c15ab41/0");
    assert_eq!(input["command"], "touch made_by_shell.txt .");
    assert_eq!(rules, &["Shell(touch made_by_shell.txt .)"]);
    // It was declined: the command shows as failed and the turn still ends normally.
    assert!(ev.iter().any(|e| matches!(e, AgentEvent::ToolResult { is_error: true, .. })));
    assert!(matches!(ev.last(), Some(AgentEvent::Done { is_error: false, .. })));

    // Answers reply to the server's own request id. This request didn't offer
    // `acceptForSession`, so "for this session" is a plain accept (Bach keeps the rule).
    let answer = |d| serde_json::from_str::<Value>(&codex_answer(suggestions, d, None).unwrap()).unwrap();
    assert_eq!(answer(Decision::Allow), json!({ "id": 0, "result": { "decision": "accept" } }));
    assert_eq!(answer(Decision::AllowSession)["result"]["decision"], "accept");
    assert_eq!(
        answer(Decision::AllowAlways)["result"]["decision"],
        json!({ "acceptWithExecpolicyAmendment": { "execpolicy_amendment": ["touch", "made_by_shell.txt", "."] } })
    );
    assert_eq!(answer(Decision::Deny)["result"]["decision"], "decline");

    // With the rule already approved this session, Bach accepts without asking.
    let (ev, sent) = codex(fixture, &["Shell(touch made_by_shell.txt .)".into()]);
    assert!(!ev.iter().any(|e| matches!(e, AgentEvent::Approval { .. })));
    assert!(sent.contains(&json!({ "id": 0, "result": { "decision": "accept" } })));
}

/// A recorded opencode turn (1.15.10): a question, a shell command that asked first, then text.
fn opencode(allowed: &[String]) -> (Vec<AgentEvent>, Vec<bach_core::adapters::opencode::Reply>) {
    let mut stream = bach_core::adapters::opencode::Stream::new("ses_f10477d98ffeJX7XYmQSN2mpi4", allowed);
    let (mut events, mut replies) = (vec![], vec![]);
    for line in include_str!("fixtures/opencode_question_permission.jsonl").lines() {
        let (ev, r) = stream.on_event(&serde_json::from_str(line).unwrap());
        events.extend(ev);
        replies.extend(r);
    }
    // Another session's events are none of this run's business.
    let mut other = bach_core::adapters::opencode::Stream::new("ses_other", &[]);
    for line in include_str!("fixtures/opencode_question_permission.jsonl").lines() {
        assert!(other.on_event(&serde_json::from_str(line).unwrap()).0.is_empty());
    }
    (events, replies)
}

#[test]
fn opencode_real_question_and_permission() {
    let (ev, replies) = opencode(&[]);
    assert!(replies.is_empty());
    assert!(matches!(&ev[0], AgentEvent::Session { model: Some(m), .. } if m == "google/gemini-3.1-flash-lite"));

    // The question, on the question card; its tool call isn't shown as well.
    let question = ev.iter().find_map(|e| match e {
        AgentEvent::Approval { tool_name, input, request_id, .. } if tool_name == "AskUserQuestion" => Some((request_id, input)),
        _ => None,
    });
    let (qid, input) = question.unwrap();
    assert_eq!(qid, "que_0efb888a2001XYSFrzu1keMd6D");
    assert_eq!(input["questions"][0]["question"], "Do you prefer red or blue?");
    assert_eq!(input["questions"][0]["options"][1]["label"], "Blue");
    assert!(!ev.iter().any(|e| matches!(e, AgentEvent::ToolUse { name, .. } if name == "question")));

    // The shell command asked first, offering a rule for commands like it.
    let (input, rules) = ev
        .iter()
        .find_map(|e| match e {
            AgentEvent::Approval { tool_name, input, rules, .. } if tool_name == "Shell" => Some((input, rules)),
            _ => None,
        })
        .unwrap();
    assert_eq!(input["command"], "echo \"Red\" > colour.txt");
    assert_eq!(rules, &["bash(echo *)"]);
    assert!(ev.iter().any(|e| matches!(e, AgentEvent::ToolUse { name, input, .. } if name == "Shell" && input["command"] == "echo \"Red\" > colour.txt")));
    assert!(ev.iter().any(|e| matches!(e, AgentEvent::ToolResult { output, is_error: false, .. } if output == "(no output)")));

    // Answered while the run watched: each answer is also reported back, but those were
    // Bach's own (Runs drops them), so they come as cancellations only this once each.
    assert_eq!(ev.iter().filter(|e| matches!(e, AgentEvent::ApprovalCancelled { .. })).count(), 2);

    assert!(ev.iter().any(|e| matches!(e, AgentEvent::Delta { text, .. } if text == "Done.")));
    assert!(ev.iter().any(|e| matches!(e, AgentEvent::Text { text, .. } if text == "Done.")));
    // The prompt itself isn't echoed back as the agent's text.
    assert!(!ev.iter().any(|e| matches!(e, AgentEvent::Text { text, .. } if text.starts_with("First use"))));
    assert!(ev.iter().any(|e| matches!(e, AgentEvent::Context { used } if *used > 14000)));
    match ev.last() {
        Some(AgentEvent::Done { cost_usd: Some(c), is_error: false }) => assert!((c - 0.00579735).abs() < 1e-9, "{c}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn opencode_session_rules_answer_for_the_user() {
    let (ev, replies) = opencode(&["bash(echo *)".into()]);
    assert!(!ev.iter().any(|e| matches!(e, AgentEvent::Approval { tool_name, .. } if tool_name == "Shell")));
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].path, "/permission/per_0efb88e8d001MNLH6vBJ6Z3gDG/reply");
    assert_eq!(replies[0].body, json!({ "reply": "once" }));
}
