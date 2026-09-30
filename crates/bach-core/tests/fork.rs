//! Forking a session from an earlier message and handing it to another agent, through the API. A
//! stand-in `claude` adds a file to its folder each turn and records what it was asked.
use bach_core::Api;
use bach_protocol::{OriginKind, Session};
use serde_json::{json, Value};
use std::{os::unix::fs::PermissionsExt, path::Path, process::Command, time::Duration};

const FAKE_CLAUDE: &str = r#"#!/bin/sh
echo "$@" >> "$BACH_TEST_DIR/args"
read first
echo "$first" >> "$BACH_TEST_DIR/prompts"
n=$(($(cat "$BACH_TEST_DIR/n" 2>/dev/null || echo 0) + 1))
echo $n > "$BACH_TEST_DIR/n"
echo turn > "file$n.txt"
echo '{"type":"system","subtype":"init","session_id":"agent'$n'","model":"claude-test"}'
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"made file'$n'"}]},"parent_tool_use_id":null}'
echo '{"type":"result","subtype":"success","is_error":false,"total_cost_usd":0}'
cat > /dev/null
"#;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

async fn log(api: &Api, id: &str) -> (Session, Vec<Value>) {
    let l = api.call("get_session", json!({ "sessionId": id })).await.unwrap();
    (serde_json::from_value(l["session"].clone()).unwrap(), l["entries"].as_array().unwrap().clone())
}

async fn idle(api: &Api, id: &str, turns: usize) -> (Session, Vec<Value>) {
    for _ in 0..200 {
        let (s, e) = log(api, id).await;
        let users = e.iter().filter(|e| e["entry"]["type"] == "user").count();
        if s.run_id.is_none() && users == turns && e.iter().any(|e| e["entry"]["event"]["type"] == "done") {
            return (s, e);
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("session {id} never went idle");
}

async fn send<T: serde::de::DeserializeOwned>(api: &Api, cmd: &str, args: Value) -> T {
    serde_json::from_value(api.call(cmd, args).await.unwrap()).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forks_from_an_earlier_message_and_hands_off() {
    let dir = std::env::temp_dir().join(format!("bach-fork-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (bin, proj) = (dir.join("bin"), dir.join("proj"));
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(bin.join("claude"), FAKE_CLAUDE).unwrap();
    std::fs::set_permissions(bin.join("claude"), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::env::set_var("PATH", format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()));
    std::env::set_var("BACH_TEST_DIR", &dir);
    for (k, v) in [("GIT_AUTHOR_NAME", "t"), ("GIT_AUTHOR_EMAIL", "t@t"), ("GIT_COMMITTER_NAME", "t"), ("GIT_COMMITTER_EMAIL", "t@t")] {
        std::env::set_var(k, v);
    }
    git(&proj, &["init", "-q", "-b", "main"]);
    std::fs::write(proj.join("base.txt"), "base\n").unwrap();
    git(&proj, &["add", "."]);
    git(&proj, &["commit", "-qm", "init"]);

    let api = Api::open(&dir.join("data/bach.db")).await.unwrap();
    let s: Session = send(&api, "start_session", json!({ "agent": "claude", "cwd": proj, "branch": "main", "worktree": true, "prompt": "first" })).await;
    idle(&api, &s.id, 1).await;
    let _: Session = send(&api, "send_message", json!({ "sessionId": s.id, "prompt": "second" })).await;
    let (source, entries) = idle(&api, &s.id, 2).await;
    let second = entries.iter().find(|e| e["entry"]["text"] == "second").unwrap()["seq"].as_u64().unwrap();
    assert!(source.workdir.as_ref().unwrap().contains("worktrees"));

    // Forked from before "second": its files are those of the first turn, and the source is as it was.
    let f: Session = send(&api, "fork_session", json!({ "sessionId": s.id, "seq": second, "prompt": "second, differently" })).await;
    let (fork, fentries) = idle(&api, &f.id, 2).await;
    let origin = fork.origin.clone().unwrap();
    assert_eq!((origin.kind, origin.session_id.as_str(), origin.seq), (OriginKind::Fork, s.id.as_str(), Some(second)));
    assert!(fork.worktree && fork.workdir != source.workdir && fork.git_branch != source.git_branch);
    let (fdir, sdir) = (Path::new(fork.workdir.as_ref().unwrap()), Path::new(source.workdir.as_ref().unwrap()));
    assert!(fdir.join("file1.txt").exists() && !fdir.join("file2.txt").exists() && fdir.join("file3.txt").exists());
    assert!(sdir.join("file2.txt").exists() && !sdir.join("file3.txt").exists());
    // The copied transcript keeps its numbering; then the new message.
    let texts: Vec<&str> = fentries.iter().filter_map(|e| e["entry"]["text"].as_str()).collect();
    assert_eq!(texts, ["first", "second, differently"]);
    assert_eq!(fentries[0]["seq"], 1);
    assert!(fork.pending_fork.is_none() || fork.agent_session_id.is_some());
    // Claude Code's own record isn't there to fork, so the agent got the transcript instead.
    let prompts = std::fs::read_to_string(dir.join("prompts")).unwrap();
    assert!(prompts.contains("earlier conversation") && prompts.contains("User: first") && prompts.contains("second, differently"));
    // The source hasn't heard of the fork.
    assert_eq!(log(&api, &s.id).await.1.len(), entries.len());

    // Forking from the first message starts a conversation from nothing, in a worktree without files.
    let first = entries[0]["seq"].as_u64().unwrap();
    let g: Session = send(&api, "fork_session", json!({ "sessionId": s.id, "seq": first })).await;
    let (g, gentries) = idle(&api, &g.id, 1).await;
    assert_eq!(gentries.iter().filter_map(|e| e["entry"]["text"].as_str()).collect::<Vec<_>>(), ["first"]);
    assert!(Path::new(g.workdir.as_ref().unwrap()).join("file4.txt").exists() && !Path::new(g.workdir.as_ref().unwrap()).join("file1.txt").exists());

    // A fork can be forked in turn (the copied message keeps its saved files).
    let h: Session = send(&api, "fork_session", json!({ "sessionId": f.id, "seq": fentries.iter().find(|e| e["entry"]["text"] == "second, differently").unwrap()["seq"] })).await;
    idle(&api, &h.id, 2).await;

    // Only a user message can be forked from.
    let agent_entry = entries.iter().find(|e| e["entry"]["type"] == "agent").unwrap()["seq"].as_u64().unwrap();
    assert!(api.call("fork_session", json!({ "sessionId": s.id, "seq": agent_entry })).await.is_err());
    assert!(api.call("fork_session", json!({ "sessionId": s.id, "seq": 999 })).await.is_err());

    // Hand-off: a draft from the transcript, then a session in the same worktree.
    let draft: String = send(&api, "handoff_summary", json!({ "sessionId": s.id })).await;
    assert!(draft.contains("## Goal\nfirst") && draft.contains("Result: made file2") && draft.contains("- file1.txt") && draft.contains("- file2.txt"));
    let edited = draft.replace("Carry on", "Please carry on");
    let hand: Session = send(&api, "handoff_session", json!({ "sessionId": s.id, "agent": "claude", "prompt": edited })).await;
    let (hand, hentries) = idle(&api, &hand.id, 1).await;
    assert_eq!(hand.origin.as_ref().map(|o| (o.kind, o.session_id.as_str())), Some((OriginKind::Handoff, s.id.as_str())));
    assert_eq!((hand.workdir.as_ref(), hand.git_branch.as_ref(), hand.worktree), (source.workdir.as_ref(), source.git_branch.as_ref(), true));
    assert!(matches!(&hentries[0]["entry"]["text"], Value::String(t) if t.contains("Please carry on")));
    assert!(api.call("handoff_session", json!({ "sessionId": s.id, "agent": "claude", "prompt": " " })).await.is_err());

    // Deleting a session drops its saved states, not its forks'.
    let refs = |id: &str| git(&proj, &["for-each-ref", &format!("refs/bach/snapshots/{id}/")]);
    assert!(!refs(&s.id).is_empty() && !refs(&f.id).is_empty());
    api.call("delete_session", json!({ "sessionId": s.id })).await.unwrap();
    assert!(refs(&s.id).is_empty() && !refs(&f.id).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}
