use super::AgentEvent;
use bach_protocol::{DeltaKind, LimitWindow, PlanUsage};
use satie::Grant;
use serde_json::{json, Value};

// ---------------------------------------------------------------------------------------------
// Steering Claude Code to Satie
// ---------------------------------------------------------------------------------------------

const GUIDANCE: &str = "Anything that must keep running after your turn ends (dev servers, simulations, watchers, \
long jobs) has to be started with the `satie` MCP tool `task_start`, not with Bash `run_in_background`, `nohup`, a trailing \
`&` or tmux: processes started those ways are stopped when the turn ends. `task_start` keeps the process running on its own, \
shows it to the user in the Tasks panel, and `task_logs`, `task_list` and `task_stop` manage it. Pass `port` when the process \
serves on one, so the call waits until it is up, and `interactive: false` when only you will use it (a server for tests or \
headless browser checks), so its ports aren't forwarded to the user's computer. For a process-compose project use `compose_start` with the compose file \
instead of running process-compose yourself: each of its processes then gets its own state and log, and `task_process` \
restarts one without the rest.";

const HOOK_DENY: &str = "Background commands are stopped when this turn ends. Start it with the satie MCP tool `task_start` \
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

/// `--mcp-config`: Satie as the `satie` MCP server, with this run's token.
fn mcp_config(grant: &Grant) -> String {
    json!({
        "mcpServers": { "satie": {
            "type": "http",
            "url": grant.url,
            "headers": { "Authorization": format!("Bearer {}", grant.token) },
        }}
    })
    .to_string()
}

/// `--settings`: the hook that stops `run_in_background` and points at Satie instead.
fn settings() -> String {
    json!({ "hooks": { "PreToolUse": [{
        "matcher": "Bash",
        "hooks": [{ "type": "command", "command": hook_command() }],
    }]}})
    .to_string()
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
    allowed_tools: &[String],
    satie: Option<&Grant>,
) -> Vec<String> {
    let mut a: Vec<String> = [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        // Text as it is written (`stream_event`s), for showing it live.
        "--include-partial-messages",
        "--permission-prompt-tool",
        "stdio",
    ]
    .map(String::from)
    .into();
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
    if let Some(grant) = satie {
        a.extend(["--mcp-config".into(), mcp_config(grant)]);
        a.extend(["--append-system-prompt".into(), GUIDANCE.into()]);
        a.extend(["--settings".into(), settings()]);
    }
    // Reading a task's state or output changes nothing, so those never need a card; starting
    // and stopping still do.
    let read_only = satie.iter().flat_map(|_| {
        [
            "mcp__satie__task_list",
            "mcp__satie__task_logs",
            "mcp__satie__port_info",
            "mcp__satie__http_check",
        ]
    });
    let allowed: Vec<String> = allowed_tools
        .iter()
        .cloned()
        .chain(read_only.map(String::from))
        .collect();
    if !allowed.is_empty() {
        a.push("--allowedTools".into());
        a.extend(allowed);
    }
    a
}

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
    })
    .into()
}

/// One line of stream-json input: a user turn. Images (`data:` URLs) go first as base64 image
/// blocks, then the text; without images the content is just the text.
pub fn user_message(prompt: &str, images: &[String]) -> String {
    let content = if images.is_empty() {
        json!(prompt)
    } else {
        let mut blocks: Vec<Value> = images.iter().filter_map(|url| image_block(url)).collect();
        if !prompt.is_empty() {
            blocks.push(json!({ "type": "text", "text": prompt }));
        }
        json!(blocks)
    };
    json!({ "type": "user", "message": { "role": "user", "content": content } }).to_string()
}

/// `data:image/png;base64,AAAA` as an image content block.
fn image_block(url: &str) -> Option<Value> {
    let (media_type, data) = url.strip_prefix("data:")?.split_once(";base64,")?;
    Some(json!({
        "type": "image",
        "source": { "type": "base64", "media_type": media_type, "data": data },
    }))
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
    use std::{
        io::Write,
        process::{Command, Stdio},
    };

    #[tokio::test]
    async fn hands_the_run_satie_with_guidance_and_a_hook() {
        let dir = std::env::temp_dir().join(format!("bach-claude-args-{}", std::process::id()));
        let satie = satie::Satie::start("127.0.0.1:0".parse().unwrap(), dir.clone())
            .await
            .unwrap();
        let grant = satie.grant(satie::Scope::default());
        let a = args(None, None, None, &[], Some(&grant));
        let after = |flag: &str| a[a.iter().position(|x| x == flag).unwrap() + 1].clone();

        let cfg: Value = serde_json::from_str(&after("--mcp-config")).unwrap();
        let server = &cfg["mcpServers"]["satie"];
        assert_eq!((server["type"].as_str(), server["url"].as_str()), (Some("http"), Some(satie.url())));
        assert_eq!(server["headers"]["Authorization"], format!("Bearer {}", grant.token));
        let prompt = after("--append-system-prompt");
        assert!(prompt.contains("task_start") && prompt.contains("run_in_background"));
        assert!(a.contains(&"mcp__satie__task_list".to_string()), "read-only tools pre-approved");
        assert!(!a.contains(&"mcp__satie__task_start".to_string()), "starting still asks");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn hook_denies_only_background_bash_calls() {
        let settings: Value = serde_json::from_str(&settings()).unwrap();
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
}
