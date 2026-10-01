use super::{mode, AgentEvent, MCP_READ_ONLY, MCP_SERVER};
use crate::attachments::{Attachment, Kind};
use bach_protocol::{DeltaKind, LimitWindow, PlanUsage};
use bach_tasks::Grant;
use serde_json::{json, Value};

// ---------------------------------------------------------------------------------------------
// Steering Claude Code to Bach's MCP server
// ---------------------------------------------------------------------------------------------

// The guidance itself is the MCP server's `instructions`, which Claude Code adds to its prompt.

const HOOK_DENY: &str = "Background commands are stopped when this turn ends. Start it with the bach MCP tool `task_start` \
instead: it keeps running independently and the user can see and stop it in the Tasks panel.";

/// A hook that denies Bash calls with `run_in_background: true` (reads the hook input on stdin).
fn hook_command() -> String {
    let deny = json!({ "hookSpecificOutput": {
        "hookEventName": "PreToolUse",
        "permissionDecision": "deny",
        "permissionDecisionReason": HOOK_DENY,
    }});
    format!(
        "if grep -Eq '\"run_in_background\"[[:space:]]*:[[:space:]]*true'; then printf '%s' '{deny}'; fi"
    )
}

/// `--mcp-config`: the `bach` MCP server, with this run's token.
fn mcp_config(grant: &Grant) -> String {
    let mut servers = serde_json::Map::new();
    servers.insert(
        super::MCP_SERVER.into(),
        json!({
            "type": "http",
            "url": grant.url,
            "headers": { "Authorization": format!("Bearer {}", grant.token) },
        }),
    );
    json!({ "mcpServers": servers }).to_string()
}

/// `--settings`: verbose output, which `stream-json` needs, and with the MCP server the hook that
/// stops `run_in_background` and points at `task_start` instead. Verbose is a setting rather than
/// `--verbose` because a wrapper may take that flag for itself (`sandbox claude --verbose`).
fn settings(mcp: bool) -> String {
    let mut settings = json!({ "verbose": true });
    if mcp {
        settings["hooks"] = json!({ "PreToolUse": [{
            "matcher": "Bash",
            "hooks": [{ "type": "command", "command": hook_command() }],
        }]});
    }
    settings.to_string()
}

// ---------------------------------------------------------------------------------------------
// Invocation
// ---------------------------------------------------------------------------------------------

/// The prompt goes over stdin (see [`user_message`]) so the process can also receive answers
/// to its permission requests (`--permission-prompt-tool stdio`).
pub fn args(
    session_id: Option<&str>,
    model: Option<&str>,
    permission_mode: Option<&str>,
    effort: Option<&str>,
    allowed_tools: &[String],
    mcp: Option<&Grant>,
) -> Vec<String> {
    let mut a: Vec<String> = [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        // Text as it is written (`stream_event`s), for showing it live.
        "--include-partial-messages",
        "--permission-prompt-tool",
        "stdio",
    ]
    .map(String::from)
    .into();
    a.extend(["--settings".into(), settings(mcp.is_some())]);
    if let Some(id) = session_id {
        a.push("--resume".into());
        a.push(id.into());
    }
    if let Some(m) = model {
        a.push("--model".into());
        a.push(m.into());
    }
    if let Some(m) = permission_mode {
        a.push("--permission-mode".into());
        a.push(m.into());
    }
    if let Some(e) = effort {
        a.push("--effort".into());
        a.push(e.into());
    }
    if let Some(grant) = mcp {
        a.extend(["--mcp-config".into(), mcp_config(grant)]);
    }
    let read_only = mcp.iter().flat_map(|_| MCP_READ_ONLY.map(|t| format!("mcp__{MCP_SERVER}__{t}")));
    let allowed: Vec<String> = allowed_tools.iter().cloned().chain(read_only).collect();
    if !allowed.is_empty() {
        a.push("--allowedTools".into());
        a.extend(allowed);
    }
    a
}

/// Claude Code keeps each conversation as `~/.claude/projects/<folder>/<session id>.jsonl`: a
/// line per message, linked to the one before it by `parentUuid`. The folder is the one the session
/// ran in, so it is looked for under all of them.
fn transcript_file(session_id: &str) -> Option<std::path::PathBuf> {
    if session_id.is_empty() || !session_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return None;
    }
    let home = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::Path::new(&h).join(".claude")))?;
    std::fs::read_dir(home.join("projects"))
        .ok()?
        .filter_map(Result::ok)
        .map(|d| d.path().join(format!("{session_id}.jsonl")))
        .find(|p| p.is_file())
}

/// What the user typed in a `user` line of the transcript file: none for tool results and other
/// messages Claude Code adds itself.
fn typed_text(line: &Value) -> Option<String> {
    if line["type"] != "user" || line["isMeta"] == true || line["isSidechain"] == true {
        return None;
    }
    match &line["message"]["content"] {
        Value::String(s) => Some(s.clone()),
        Value::Array(parts) if !parts.iter().any(|p| p["type"] == "tool_result") => Some(
            parts
                .iter()
                .filter(|p| p["type"] == "text")
                .filter_map(|p| p["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        _ => None,
    }
}

/// The message to continue a copy of session `session_id`'s conversation from so that it ends
/// just before the user's `turn`th message (counting from 0): the last assistant message before it.
/// `text` is that message's text, checked against the transcript file so the two agree on which
/// one it is.
pub fn fork_point(session_id: &str, turn: usize, text: &str) -> Result<String, String> {
    let file = transcript_file(session_id).ok_or("Claude Code's record of this conversation isn't on this machine.")?;
    let lines: Vec<Value> = std::fs::read_to_string(&file)
        .map_err(|e| e.to_string())?
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let (mut seen, mut last_assistant) = (0, None);
    for line in &lines {
        if let Some(typed) = typed_text(line) {
            if seen == turn {
                if typed.trim() != text.trim() {
                    return Err("Claude Code's record of this conversation doesn't match the transcript.".into());
                }
                return last_assistant.ok_or_else(|| "There is nothing before that message to continue from.".to_string());
            }
            seen += 1;
        } else if line["type"] == "assistant" && line["isSidechain"] != true {
            last_assistant = line["uuid"].as_str().map(String::from);
        }
    }
    Err("Claude Code's record of this conversation doesn't have that message.".into())
}

/// Asks a short-lived Claude Code for its skills and slash commands (an `initialize` control
/// request, which needs no prompt, so this costs no tokens).
pub async fn list_skills(cwd: Option<&str>) -> Result<Vec<bach_protocol::SkillInfo>, String> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let program = crate::wrapper::command("claude");
    let mut cmd = tokio::process::Command::new(&program[0]);
    cmd.args(&program[1..])
        .args(["-p", "--input-format", "stream-json", "--output-format", "stream-json"])
        .args(["--settings".to_string(), settings(false)])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let mut child = cmd.spawn().map_err(|e| format!("couldn't start `claude`: {e}"))?;
    // Held open until the list arrives: closing it would end the run first.
    let mut stdin = child.stdin.take().expect("piped");
    let request = json!({ "type": "control_request", "request_id": "skills", "request": { "subtype": "initialize" } });
    stdin
        .write_all(format!("{request}\n").as_bytes())
        .await
        .map_err(|e| format!("couldn't ask Claude Code for its skills: {e}"))?;
    let mut out = BufReader::new(child.stdout.take().expect("piped")).lines();
    let event = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        while let Ok(Some(line)) = out.next_line().await {
            let v: Value = serde_json::from_str(&line).unwrap_or_default();
            if v["type"] == "control_response" && v["response"]["request_id"] == "skills" {
                return Some(v["response"]["response"].clone());
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
    .ok_or("Claude Code didn't list its skills.")?;
    drop(stdin);
    Ok(skills_from(&event))
}

/// The commands in an `initialize` response, without Claude Code's internal ones (`__…`).
fn skills_from(event: &Value) -> Vec<bach_protocol::SkillInfo> {
    let text = |v: &Value| v.as_str().unwrap_or_default().to_string();
    event["commands"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| bach_protocol::SkillInfo {
            name: text(&c["name"]),
            description: text(&c["description"]),
            argument_hint: text(&c["argumentHint"]),
        })
        .filter(|c| !c.name.is_empty() && !c.name.starts_with("__"))
        .collect()
}

/// Claude Code's permission modes (`--permission-mode`), worded as its own picker does. "default"
/// passes none: Claude Code asks before changes.
pub fn permission_modes() -> Vec<bach_protocol::PermissionModeInfo> {
    vec![
        mode("auto", "Auto", "Claude handles permission decisions", false),
        mode("default", "Manual", "Always ask before making changes", true),
        mode("acceptEdits", "Accept edits", "Automatically accept all file edits", false),
        mode("plan", "Plan", "Create a plan before making changes", false),
        mode("bypassPermissions", "Bypass permissions", "Run everything without asking", false),
    ]
}

/// Claude Code's `--effort` levels.
const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// Claude Code's model aliases (`--model`), which follow the latest of each family.
pub fn models() -> Vec<bach_protocol::ModelInfo> {
    [
        ("opus", "Opus", "Most capable, for hard problems"),
        ("sonnet", "Sonnet", "Fast and capable for everyday work"),
        ("haiku", "Haiku", "Fastest, for small tasks"),
    ]
    .map(|(id, name, description)| bach_protocol::ModelInfo {
        id: id.into(),
        name: name.into(),
        description: description.into(),
        is_default: false,
        efforts: EFFORTS.map(String::from).into(),
        default_effort: None,
    })
    .into()
}

/// One line of stream-json input: a user turn. Attachments (`data:` URLs) go first, images as
/// image blocks and PDFs and text files as document blocks, then the text; without attachments
/// the content is just the text.
pub fn user_message(prompt: &str, attachments: &[String]) -> String {
    let content = if attachments.is_empty() {
        json!(prompt)
    } else {
        let mut blocks: Vec<Value> = attachments.iter().filter_map(|url| attachment_block(url)).collect();
        if !prompt.is_empty() {
            blocks.push(json!({ "type": "text", "text": prompt }));
        }
        json!(blocks)
    };
    json!({ "type": "user", "message": { "role": "user", "content": content } }).to_string()
}

/// `data:image/png;base64,AAAA` as an image content block, a PDF or text file as a document.
fn attachment_block(url: &str) -> Option<Value> {
    let a = Attachment::parse(url)?;
    let mut block = match a.kind {
        Kind::Image => json!({
            "type": "image",
            "source": { "type": "base64", "media_type": a.mime, "data": a.data },
        }),
        Kind::Pdf => json!({
            "type": "document",
            "source": { "type": "base64", "media_type": a.mime, "data": a.data },
        }),
        Kind::Text => json!({
            "type": "document",
            "source": { "type": "text", "media_type": "text/plain", "data": String::from_utf8_lossy(&a.bytes()?) },
        }),
    };
    if let Some(name) = a.name {
        block["title"] = json!(name);
    }
    Some(block)
}

/// Folders the agent offers to add access to (a command reaching outside the project).
fn directories_from(suggestions: &Value) -> Vec<String> {
    suggestions
        .as_array()
        .into_iter()
        .flatten()
        .filter(|s| s["type"] == "addDirectories")
        .flat_map(|s| s["directories"].as_array().into_iter().flatten())
        .filter_map(|d| d.as_str().map(str::to_string))
        .collect()
}

/// `Bash(tmux ls *)`-style rule strings from the agent's suggested permission updates.
fn rules_from(suggestions: &Value) -> Vec<String> {
    suggestions
        .as_array()
        .into_iter()
        .flatten()
        .filter(|s| s["type"] == "addRules" && s["behavior"] == "allow")
        .flat_map(|s| s["rules"].as_array().into_iter().flatten())
        .filter_map(|r| {
            let tool = r["toolName"].as_str()?;
            Some(match r["ruleContent"].as_str() {
                Some(c) => format!("{tool}({c})"),
                None => tool.to_string(),
            })
        })
        .collect()
}

/// Tokens in the conversation after a message: everything sent (fresh or cached) plus the reply.
fn context_used(usage: &Value) -> Option<u64> {
    let n = |k: &str| usage[k].as_u64().unwrap_or(0);
    usage.is_object().then(|| {
        n("input_tokens")
            + n("cache_read_input_tokens")
            + n("cache_creation_input_tokens")
            + n("output_tokens")
    })
}

/// A `rate_limit_event`'s windows (`five_hour`, `seven_day`, ...); times in ms.
fn limits(info: &Value) -> Option<PlanUsage> {
    let windows = info["unifiedWindows"]
        .as_object()?
        .iter()
        .filter_map(|(name, w)| {
            Some((
                name.clone(),
                LimitWindow {
                    utilization: w["utilization"].as_f64()?,
                    resets_at: w["resetsAt"].as_i64().map(|s| s * 1000),
                },
            ))
        })
        .collect();
    Some(PlanUsage {
        status: info["status"].as_str().unwrap_or("allowed").to_string(),
        windows,
        observed_at: crate::store::now_ms(),
    })
}

/// Tool results are either a string or a list of content blocks; keep the readable text.
fn content_text(content: &Value) -> String {
    match content {
        Value::String(t) => t.clone(),
        Value::Array(blocks) => {
            let texts: Vec<&str> = blocks.iter().filter_map(|b| b["text"].as_str()).collect();
            if texts.is_empty() {
                if blocks.iter().all(|b| b["type"] == "image") {
                    return String::new();
                }
                content.to_string()
            } else {
                texts.join("\n")
            }
        }
        other => other.to_string(),
    }
}

/// Image blocks of a tool result as `data:` URLs (only base64 sources; the readable text is
/// `content_text`). A screenshot from a browser tool arrives this way.
fn content_images(content: &Value) -> Vec<String> {
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| b["type"] == "image" && b["source"]["type"] == "base64")
        .filter_map(|b| {
            let media = b["source"]["media_type"].as_str()?;
            let data = b["source"]["data"].as_str()?;
            media
                .starts_with("image/")
                .then(|| format!("data:{media};base64,{data}"))
        })
        .collect()
}

pub fn parse(v: &Value) -> Vec<AgentEvent> {
    let s = |x: &Value| x.as_str().unwrap_or_default().to_string();
    let opt = |x: &Value| x.as_str().map(str::to_string);
    let num = |x: &Value| x.as_u64();
    match v["type"].as_str() {
        // Only the main conversation streams; the whole block follows as a message. Thinking
        // often arrives without its text (summarised or redacted), leaving nothing to show.
        Some("stream_event") => {
            let e = &v["event"];
            let (kind, text) = match e["delta"]["type"].as_str() {
                Some("text_delta") => (DeltaKind::Text, s(&e["delta"]["text"])),
                Some("thinking_delta") => (DeltaKind::Thinking, s(&e["delta"]["thinking"])),
                _ => return vec![],
            };
            if e["type"] != "content_block_delta" || !v["parent_tool_use_id"].is_null() || text.is_empty() {
                return vec![];
            }
            vec![AgentEvent::Delta {
                id: format!("{}", e["index"]),
                kind,
                text,
            }]
        }
        Some("system") => match v["subtype"].as_str() {
            Some("init") => vec![AgentEvent::Session {
                id: s(&v["session_id"]),
                model: opt(&v["model"]),
            }],
            // Sub-agent / background task lifecycle, keyed by the tool call that spawned it.
            Some(kind @ ("task_started" | "task_progress" | "task_notification")) => {
                let Some(id) = opt(&v["tool_use_id"]) else {
                    return vec![];
                };
                let usage = &v["usage"];
                vec![AgentEvent::Task {
                    id,
                    status: match kind {
                        "task_started" => Some("running".into()),
                        "task_notification" => opt(&v["status"]),
                        _ => None,
                    },
                    title: (kind == "task_started")
                        .then(|| opt(&v["description"]))
                        .flatten(),
                    agent_type: opt(&v["subagent_type"]),
                    activity: (kind == "task_progress")
                        .then(|| opt(&v["description"]))
                        .flatten(),
                    tool_uses: num(&usage["tool_uses"]),
                    tokens: num(&usage["total_tokens"]),
                    duration_ms: num(&usage["duration_ms"]),
                    summary: opt(&v["summary"]),
                    background: v["is_backgrounded"].as_bool(),
                }]
            }
            _ => vec![],
        },
        Some(kind @ ("assistant" | "user")) => {
            // Set for everything a sub-agent does; the id is the tool call that spawned it.
            let parent = opt(&v["parent_tool_use_id"]);
            let mut events: Vec<AgentEvent> = v["message"]["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|block| match block["type"].as_str()? {
                    // "user" text is just the prompt handed to a sub-agent.
                    "text" if kind == "assistant" => Some(AgentEvent::Text {
                        text: s(&block["text"]),
                        parent: parent.clone(),
                    }),
                    // Redacted thinking arrives empty; a sub-agent's thinking is noise.
                    "thinking" if parent.is_none() => Some(s(&block["thinking"]))
                        .filter(|t| !t.is_empty())
                        .map(|text| AgentEvent::Thinking { text }),
                    "tool_use" => Some(AgentEvent::ToolUse {
                        id: s(&block["id"]),
                        name: s(&block["name"]),
                        input: block["input"].clone(),
                        parent: parent.clone(),
                    }),
                    "tool_result" => Some(AgentEvent::ToolResult {
                        id: s(&block["tool_use_id"]),
                        output: content_text(&block["content"]),
                        is_error: block["is_error"].as_bool().unwrap_or(false),
                        images: content_images(&block["content"]),
                        parent: parent.clone(),
                    }),
                    _ => None,
                })
                .collect();
            // The session's own messages say how full its context is; a sub-agent has its own.
            if kind == "assistant" && parent.is_none() {
                events.extend(context_used(&v["message"]["usage"]).map(|used| AgentEvent::Context { used }));
            }
            events
        }
        Some("rate_limit_event") => limits(&v["rate_limit_info"])
            .map(|usage| AgentEvent::Limits { usage })
            .into_iter()
            .collect(),
        Some("control_request") if v["request"]["subtype"] == "can_use_tool" => {
            let r = &v["request"];
            vec![AgentEvent::Approval {
                request_id: s(&v["request_id"]),
                tool_use_id: opt(&r["tool_use_id"]),
                tool_name: s(&r["tool_name"]),
                input: r["input"].clone(),
                description: opt(&r["description"]),
                reason: opt(&r["decision_reason"]),
                rules: rules_from(&r["permission_suggestions"]),
                directories: directories_from(&r["permission_suggestions"]),
                suggestions: r["permission_suggestions"].clone(),
            }]
        }
        Some("control_cancel_request") => vec![AgentEvent::ApprovalCancelled {
            request_id: s(&v["request_id"]),
        }],
        Some("result") => {
            let windows: std::collections::BTreeMap<String, u64> = v["modelUsage"]
                .as_object()
                .into_iter()
                .flatten()
                .filter_map(|(model, u)| Some((model.clone(), u["contextWindow"].as_u64()?)))
                .collect();
            let mut events = vec![];
            if !windows.is_empty() {
                events.push(AgentEvent::ContextWindows { windows });
            }
            events.push(AgentEvent::Done {
                cost_usd: v["total_cost_usd"].as_f64(),
                is_error: v["is_error"].as_bool().unwrap_or(false),
            });
            events
        }
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skills_come_from_the_initialize_response() {
        let event = json!({ "commands": [
            { "name": "code-review", "description": "Review the diff", "argumentHint": "[level]", "builtin": true },
            { "name": "__remote-workflow", "description": "", "argumentHint": "" },
            { "name": "docs", "description": "Docs" },
        ]});
        let skills = skills_from(&event);
        let names: Vec<_> = skills.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["code-review", "docs"]);
        assert_eq!(skills[0].argument_hint, "[level]");
        assert_eq!(skills[1].argument_hint, "");
    }
    use std::{
        io::Write,
        process::{Command, Stdio},
    };

    #[tokio::test]
    async fn hands_the_run_the_bach_server_and_a_hook() {
        let dir = std::env::temp_dir().join(format!("bach-claude-args-{}", std::process::id()));
        let tasks = bach_tasks::Tasks::start("127.0.0.1:0".parse().unwrap(), dir.clone())
            .await
            .unwrap();
        let grant = tasks.grant(bach_tasks::Scope::default());
        let a = args(None, None, None, None, &[], Some(&grant));
        let after = |flag: &str| a[a.iter().position(|x| x == flag).unwrap() + 1].clone();

        let cfg: Value = serde_json::from_str(&after("--mcp-config")).unwrap();
        let server = &cfg["mcpServers"]["bach"];
        assert_eq!((server["type"].as_str(), server["url"].as_str()), (Some("http"), Some(tasks.url())));
        assert_eq!(server["headers"]["Authorization"], format!("Bearer {}", grant.token));
        // The system prompt is left to a wrapper; the guidance comes with the tools.
        assert!(!a.contains(&"--append-system-prompt".to_string()));
        assert!(a.contains(&"mcp__bach__task_list".to_string()), "read-only tools pre-approved");
        assert!(!a.contains(&"mcp__bach__task_start".to_string()), "starting still asks");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn hook_denies_only_background_bash_calls() {
        let settings: Value = serde_json::from_str(&settings(true)).unwrap();
        assert_eq!(settings["verbose"], true);
        let hook = &settings["hooks"]["PreToolUse"][0];
        assert_eq!(hook["matcher"], "Bash");
        let cmd = hook["hooks"][0]["command"].as_str().unwrap();

        let run = |input: &str| {
            let mut child = Command::new("sh")
                .arg("-c")
                .arg(cmd)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.as_bytes())
                .unwrap();
            String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap()
        };
        let bg = run(
            r#"{"tool_name":"Bash","tool_input":{"command":"sleep 9","run_in_background":true}}"#,
        );
        let v: Value = serde_json::from_str(&bg).unwrap();
        assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "deny");
        assert!(v["hookSpecificOutput"]["permissionDecisionReason"]
            .as_str()
            .unwrap()
            .contains("task_start"));
        let spaced = run(r#"{"tool_input": {"run_in_background" : true}}"#);
        assert!(spaced.contains("deny"), "whitespace variants too");
        assert_eq!(
            run(r#"{"tool_name":"Bash","tool_input":{"command":"ls","run_in_background":false}}"#),
            ""
        );
        assert_eq!(
            run(r#"{"tool_name":"Bash","tool_input":{"command":"ls"}}"#),
            ""
        );
    }

    #[test]
    fn forks_continue_from_the_assistant_message_before_the_user_one() {
        let home = std::env::temp_dir().join(format!("bach-claude-home-{}", std::process::id()));
        let project = home.join("projects/-some-folder");
        std::fs::create_dir_all(&project).unwrap();
        let lines = [
            r#"{"type":"queue-operation"}"#,
            r#"{"type":"user","uuid":"u1","message":{"content":"first"}}"#,
            r#"{"type":"assistant","uuid":"a1","message":{"content":[{"type":"text","text":"ok"}]}}"#,
            r#"{"type":"user","uuid":"t1","message":{"content":[{"type":"tool_result","tool_use_id":"x","content":"r"}]}}"#,
            r#"{"type":"assistant","uuid":"a2","isSidechain":true,"message":{"content":[]}}"#,
            r#"{"type":"assistant","uuid":"a3","message":{"content":[{"type":"text","text":"done"}]}}"#,
            r#"{"type":"user","uuid":"u2","message":{"content":[{"type":"text","text":"second"}]}}"#,
        ];
        std::fs::write(project.join("abc-123.jsonl"), lines.join("\n")).unwrap();
        std::env::set_var("CLAUDE_CONFIG_DIR", &home);
        // Tool results and sub-agent messages aren't turns.
        assert_eq!(fork_point("abc-123", 1, "second").unwrap(), "a3");
        // It is the user's message that is checked, so a transcript out of step is noticed.
        assert!(fork_point("abc-123", 1, "something else").is_err());
        assert!(fork_point("abc-123", 0, "first").is_err(), "nothing comes before the first message");
        assert!(fork_point("abc-123", 2, "third").is_err());
        assert!(fork_point("missing", 1, "second").is_err() && fork_point("../x", 1, "second").is_err());
        let _ = std::fs::remove_dir_all(&home);
    }
}
