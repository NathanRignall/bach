//! Agents starting sessions: the `start_session` tool on the `bach` MCP server, as a running
//! agent calls it with its run's token. A stand-in `claude` keeps each run going until stopped.
use bach_core::Api;
use bach_protocol::Session;
use serde_json::{json, Value};
use std::{os::unix::fs::PermissionsExt, path::Path, process::Command, time::Duration};

const FAKE_CLAUDE: &str = r#"#!/bin/sh
echo "$@" >> "$BACH_TEST_DIR/args"
while [ $# -gt 0 ]; do
  if [ "$1" = --mcp-config ]; then printf '%s' "$2" > "$BACH_TEST_DIR/mcp"; fi
  shift
done
read first
echo '{"type":"system","subtype":"init","session_id":"s1","model":"claude-test"}'
cat > /dev/null
"#;

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "init.defaultBranch=main"])
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

async fn wait_for(what: &str, mut cond: impl FnMut() -> bool) {
    for _ in 0..100 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for: {what}");
}

/// One JSON-RPC call to the MCP server described by `cfg` (Claude Code's `--mcp-config`).
async fn mcp(cfg: &Value, method: &str, params: Value) -> Value {
    let server = &cfg["mcpServers"]["bach"];
    reqwest::Client::new()
        .post(server["url"].as_str().unwrap())
        .header("Authorization", server["headers"]["Authorization"].as_str().unwrap())
        .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }))
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap()["result"]
        .clone()
}

async fn start_session(cfg: &Value, args: Value) -> (bool, String) {
    let r = mcp(cfg, "tools/call", json!({ "name": "start_session", "arguments": args })).await;
    (r["isError"].as_bool().unwrap(), r["content"][0]["text"].as_str().unwrap().to_string())
}

async fn sessions(api: &Api) -> Vec<Session> {
    serde_json::from_value(api.call("list_sessions", json!({})).await.unwrap()).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_running_agent_starts_a_session_in_its_project() {
    let dir = std::env::temp_dir().join(format!("bach-spawn-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (bin, proj) = (dir.join("bin"), dir.join("proj"));
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(bin.join("claude"), FAKE_CLAUDE).unwrap();
    std::fs::set_permissions(bin.join("claude"), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::env::set_var("PATH", format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()));
    std::env::set_var("BACH_TEST_DIR", &dir);
    git(&proj, &["init", "-q"]);
    git(&proj, &["commit", "-q", "--allow-empty", "-m", "start"]);

    let api = Api::open(&dir.join("data/bach.db")).await.unwrap();
    let parent: Session = serde_json::from_value(
        api.call(
            "start_session",
            json!({ "agent": "claude", "cwd": proj, "prompt": "main task", "modelChoice": "sonnet", "permissionMode": "acceptEdits" }),
        )
        .await
        .unwrap(),
    )
    .unwrap();
    wait_for("the agent to start", || dir.join("mcp").exists()).await;
    let cfg: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("mcp")).unwrap()).unwrap();

    // One server, named `bach`, with bach-tasks' tools and Bach's.
    let init = mcp(&cfg, "initialize", json!({ "protocolVersion": "2025-06-18" })).await;
    assert_eq!(init["serverInfo"]["name"], "bach");
    let told = init["instructions"].as_str().unwrap();
    assert!(told.contains("task_start") && told.contains("start_session"), "{told}");
    let tools = mcp(&cfg, "tools/list", json!({})).await;
    let names: Vec<&str> = tools["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"task_start") && names.contains(&"start_session"), "{names:?}");

    // Asking badly is explained.
    assert_eq!(start_session(&cfg, json!({ "prompt": " " })).await.0, true);
    let (err, text) = start_session(&cfg, json!({ "prompt": "x", "agent": "gpt" })).await;
    assert!(err && text.contains("claude, codex or opencode"), "{text}");
    assert_eq!(sessions(&api).await.len(), 1);

    // By default: same project and agent settings, in a worktree of its own.
    let (err, text) = start_session(&cfg, json!({ "prompt": "Fix the flaky login test\n\nDetails..." })).await;
    assert!(!err, "{text}");
    let id = text.strip_prefix("Started session ").unwrap().split(' ').next().unwrap().to_string();
    let child = sessions(&api).await.into_iter().find(|s| s.id == id).expect("listed");
    assert_eq!((child.cwd.as_str(), child.agent), (parent.cwd.as_str(), parent.agent));
    assert_eq!(child.model_choice.as_deref(), Some("sonnet"));
    assert_eq!(child.permission_mode.as_deref(), Some("acceptEdits"));
    assert!(child.worktree && child.git_branch.as_deref().unwrap().starts_with("bach/fix-the-flaky-login-test-"));
    assert_ne!(child.workdir, parent.workdir);
    assert!(child.run_id.is_some(), "its agent is working");
    assert!(text.contains(child.workdir.as_deref().unwrap()), "{text}");
    wait_for("the new session's agent to start", || {
        std::fs::read_to_string(dir.join("args")).unwrap_or_default().lines().count() == 2
    })
    .await;

    // Asked for: another agent's settings don't carry over, and no worktree means the project folder.
    let (err, text) = start_session(&cfg, json!({ "prompt": "look around", "model": "opus", "worktree": false })).await;
    assert!(!err, "{text}");
    let here = sessions(&api).await.into_iter().find(|s| s.title == "look around").unwrap();
    assert_eq!((here.worktree, here.workdir.as_deref()), (false, Some(proj.to_str().unwrap())));
    assert_eq!(here.model_choice.as_deref(), Some("opus"));

    for s in sessions(&api).await {
        let _ = api.call("stop_session", json!({ "sessionId": s.id })).await;
    }
    // Once the run is over, its token no longer starts anything.
    wait_for("the grants to go", || api.tasks().active_grants() == 0).await;
    let _ = std::fs::remove_dir_all(dir);
}
