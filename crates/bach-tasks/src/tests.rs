//! bach-tasks' tests: real processes, real ports, a real MCP client over HTTP.
    use super::*;
    use std::path::Path;
    use crate::process::{session_of, signal_session, task_alive};
    use crate::sys;
    use serde_json::{json, Value};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    pub(super) static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    pub(super) fn tmp(label: &str) -> PathBuf {
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let d = std::env::temp_dir().join(format!("bach-tasks-{label}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    pub(super) async fn tasks_in(dir: &Path) -> Tasks {
        Tasks::start_with(
            "127.0.0.1:0".parse().unwrap(),
            dir.join("tasks"),
            Duration::from_millis(50),
        )
        .await
        .unwrap()
    }

    fn req(command: &str, cwd: &Path) -> StartTask {
        StartTask {
            command: command.into(),
            cwd: Some(cwd.to_string_lossy().into()),
            ..Default::default()
        }
    }

    pub(super) async fn until(what: &str, mut cond: impl FnMut() -> bool) {
        for _ in 0..100 {
            if cond() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("timed out waiting for: {what}");
    }

    fn status(s: &Tasks, id: &str) -> TaskStatus {
        s.get(id).unwrap().status
    }

    /// Any process whose command line contains `needle`.
    fn procs_matching(needle: &str) -> Vec<u32> {
        sys::all_pids()
            .into_iter()
            .filter(|pid| {
                sys::argv(*pid).join(" ").contains(needle)
                    && sys::stat(*pid).is_some_and(|s| !s.zombie)
                    && *pid != std::process::id()
            })
            .collect()
    }

    #[test]
    fn presents_recognisable_ports_first() {
        // A dev stack: named ports plus random internal ones -> only the named ones.
        assert_eq!(
            presentable_ports(vec![4001, 8787, 9229, 34865, 40000]),
            vec![4001, 8787, 9229]
        );
        // Only random ports: those are the point of the task, so they stay.
        assert_eq!(presentable_ports(vec![40001, 40000]), vec![40001, 40000]);
        assert_eq!(presentable_ports(vec![]), Vec::<u16>::new());
    }

    #[tokio::test]
    async fn runs_detached_and_records_output_and_exit_code() {
        let dir = tmp("exit");
        let tasks = tasks_in(&dir).await;

        let ok = tasks
            .start_task(req("echo hello; echo oops >&2", &dir))
            .unwrap();
        until("exit 0", || status(&tasks, &ok.id) == TaskStatus::Exited).await;
        let out = tasks.logs(&ok.id, 10, None).unwrap();
        assert!(
            out.contains("$ echo hello") && out.contains("hello") && out.contains("oops"),
            "{out}"
        );
        assert_eq!(tasks.get(&ok.id).unwrap().exit_code, Some(0));

        let bad = tasks.start_task(req("exit 3", &dir)).unwrap();
        until("exit 3", || status(&tasks, &bad.id) == TaskStatus::Failed).await;
        assert_eq!(tasks.get(&bad.id).unwrap().exit_code, Some(3));

        // Tools are asked for colour, since the log viewers show it.
        let env = tasks.start_task(req("echo \"$FORCE_COLOR $CLICOLOR_FORCE\"", &dir)).unwrap();
        until("env", || status(&tasks, &env.id) == TaskStatus::Exited).await;
        assert!(tasks.logs(&env.id, 1, None).unwrap().ends_with("1 1"));

        // Only the last lines are returned.
        let many = tasks.start_task(req("seq 1 100", &dir)).unwrap();
        until("seq", || status(&tasks, &many.id) == TaskStatus::Exited).await;
        assert_eq!(tasks.logs(&many.id, 3, None).unwrap(), "98\n99\n100");

        // It ran in its own session, not ours.
        let pid = tasks.start_task(req("sleep 4701", &dir)).unwrap();
        let (sid_of_task, sid_ours) = (
            session_of(pid.pid).unwrap(),
            session_of(std::process::id()).unwrap(),
        );
        assert_ne!(sid_of_task, sid_ours);
        assert_eq!(sid_of_task, pid.pid, "it leads its own session");
        tasks.stop_task(&pid.id).await.unwrap();

        assert!(tasks.start_task(req("", &dir)).is_err());
        assert!(tasks
            .start_task(req("true", Path::new("/definitely/not/here")))
            .is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn stop_kills_everything_the_task_started() {
        let dir = tmp("stop");
        let tasks = tasks_in(&dir).await;
        let t = tasks
            .start_task(req("sleep 4702 & sleep 4702 & wait", &dir))
            .unwrap();
        until("children up", || procs_matching("sleep 4702").len() >= 2).await;

        let stopped = tasks.stop_task(&t.id).await.unwrap();
        assert_eq!(stopped.status, TaskStatus::Stopped);
        until("all gone", || procs_matching("sleep 4702").is_empty()).await;
        // The monitor doesn't reclassify a stopped task as lost.
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(status(&tasks, &t.id), TaskStatus::Stopped);
        assert!(
            tasks.stop_task(&t.id).await.is_ok(),
            "stopping twice is harmless"
        );

        tasks.remove_task(&t.id).unwrap();
        assert!(tasks.get(&t.id).is_none() && !Path::new(&t.log_path).exists());
        let running = tasks.start_task(req("sleep 4703", &dir)).unwrap();
        assert!(
            tasks.remove_task(&running.id).is_err(),
            "can't remove a running task"
        );
        tasks.stop_task(&running.id).await.unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn starting_the_same_command_again_replaces_its_finished_runs() {
        let dir = tmp("again");
        let tasks = tasks_in(&dir).await;
        let first = tasks.start_task(req("exit 0", &dir)).unwrap();
        until("first ended", || status(&tasks, &first.id) == TaskStatus::Exited).await;
        // A running one, and a finished one of a different command, stay.
        let running = tasks.start_task(req("sleep 4709", &dir)).unwrap();
        let other = tasks.start_task(req("exit 1", &dir)).unwrap();
        until("other ended", || status(&tasks, &other.id) == TaskStatus::Failed).await;

        let again = tasks.start_task(req("exit 0", &dir)).unwrap();
        assert!(tasks.get(&first.id).is_none(), "the finished run is replaced");
        assert!(!Path::new(&first.log_path).exists(), "and its log is gone");
        assert!(tasks.get(&again.id).is_some());
        assert!(tasks.get(&running.id).is_some() && tasks.get(&other.id).is_some());

        // The same command in another folder is a different thing.
        let elsewhere = tmp("again-elsewhere");
        let there = tasks.start_task(req("exit 0", &elsewhere)).unwrap();
        assert!(tasks.get(&again.id).is_some());
        assert!(tasks.get(&there.id).is_some());

        tasks.stop_task(&running.id).await.unwrap();
        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::remove_dir_all(elsewhere);
    }

    #[tokio::test]
    async fn a_restart_forgets_runs_superseded_before_the_rule_existed() {
        let dir = tmp("prune");
        let before = tasks_in(&dir).await;
        let old = before.start_task(req("exit 0", &dir)).unwrap();
        until("old ended", || status(&before, &old.id) == TaskStatus::Exited).await;
        // Put a second, later run beside it by hand, as an older version would have left it.
        let mut newer = old.clone();
        newer.id = new_task_id();
        newer.started_at = old.started_at + 1;
        before.import(vec![newer.clone()]);
        assert!(before.get(&old.id).is_some() && before.get(&newer.id).is_some());
        drop(before);

        let after = tasks_in(&dir).await;
        assert!(after.get(&old.id).is_none(), "the superseded run is forgotten on startup");
        assert!(after.get(&newer.id).is_some(), "the latest run stays");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn tasks_survive_a_restart_and_are_reconciled() {
        let dir = tmp("restart");
        let before = tasks_in(&dir).await;
        let long = before.start_task(req("sleep 4704", &dir)).unwrap();
        let short = before.start_task(req("sleep 0.3; exit 7", &dir)).unwrap();
        // A task whose process vanished without a trace.
        let gone = before.start_task(req("sleep 4705", &dir)).unwrap();
        // Simulate a restart: nothing of `before` is consulted from here on.
        let running_pids = (long.pid, gone.pid);
        drop(before);
        signal_session(running_pids.1, libc::SIGKILL);
        std::fs::remove_file(dir.join("tasks").join(format!("{}.exit", gone.id))).ok();
        tokio::time::sleep(Duration::from_millis(600)).await;

        let after = tasks_in(&dir).await;
        assert_eq!(
            status(&after, &long.id),
            TaskStatus::Running,
            "still running, and found again"
        );
        assert!(task_alive(
            long.pid,
            after.get(&long.id).unwrap().start_ticks
        ));
        assert_eq!(status(&after, &short.id), TaskStatus::Failed);
        assert_eq!(
            after.get(&short.id).unwrap().exit_code,
            Some(7),
            "exit code read from its file"
        );
        until("lost", || status(&after, &gone.id) == TaskStatus::Lost).await;

        // The new instance can stop what the old one started.
        after.stop_task(&long.id).await.unwrap();
        until("stopped", || !task_alive(long.pid, None)).await;
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn reports_listening_ports_and_waits_for_them() {
        let dir = tmp("ports");
        let tasks = tasks_in(&dir).await;
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let t = tasks
            .start_task(req(
                &format!("python3 -m http.server {port} --bind 127.0.0.1"),
                &dir,
            ))
            .unwrap();
        tasks
            .wait_ready(&t.id, &[port], None, Duration::from_secs(10))
            .await;
        let view = tasks.view(&t.id).unwrap();
        assert_eq!(view.ports, vec![port], "{view:?}");
        assert!(Tasks::describe(&view).contains(&format!("listening on {port}")));
        tasks.stop_task(&t.id).await.unwrap();
        assert!(tasks.view(&t.id).unwrap().ports.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn the_task_view_hides_random_ports_but_keeps_named_ones() {
        let dir = tmp("presentable");
        let tasks = tasks_in(&dir).await;
        // A named port (below the ephemeral range) that is free right now.
        let named = (20000u16..30000)
            .find(|p| std::net::TcpListener::bind(("127.0.0.1", *p)).is_ok())
            .expect("a free port");
        let script = format!(
            "import socket,time\na=socket.socket(); a.bind(('127.0.0.1',{named})); a.listen()\nb=socket.socket(); b.bind(('127.0.0.1',0)); b.listen()\nc=socket.socket(); c.bind(('127.0.0.1',0)); c.listen()\ntime.sleep(60)\n"
        );
        std::fs::write(dir.join("two.py"), script).unwrap();
        let t = tasks.start_task(req("python3 two.py", &dir)).unwrap();

        // The process really has three listeners...
        until("three listeners", || ports_of_session(t.pid).len() == 3).await;
        // ...but only the recognisable one is presented, in the view and in what the agent is told.
        let view = tasks.view(&t.id).unwrap();
        assert_eq!(view.ports, vec![named], "{view:?}");
        assert!(Tasks::describe(&view).contains(&format!("listening on {named}")));
        // The first line is state + ports; any details follow on their own lines.
        let first = Tasks::describe(&view);
        let first = first.lines().next().unwrap();
        assert!(
            first.ends_with(&format!("listening on {named}")),
            "the description lists just the named port: {first}"
        );
        tasks.stop_task(&t.id).await.unwrap();

        // A task that only has random ports keeps them: they are the point of it.
        std::fs::write(dir.join("rand.py"), "import socket,time\nb=socket.socket(); b.bind(('127.0.0.1',0)); b.listen()\ntime.sleep(60)\n").unwrap();
        let r = tasks.start_task(req("python3 rand.py", &dir)).unwrap();
        until("random listener", || ports_of_session(r.pid).len() == 1).await;
        assert_eq!(tasks.view(&r.id).unwrap().ports.len(), 1);
        tasks.stop_task(&r.id).await.unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    // ----- MCP over HTTP -------------------------------------------------------------------

    /// Minimal HTTP/1.1 client: (status, body).
    async fn post(url: &str, token: Option<&str>, body: &Value) -> (u16, String) {
        let addr = url.trim_start_matches("http://").trim_end_matches("/mcp");
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        let body = body.to_string();
        let auth = token
            .map(|t| format!("Authorization: Bearer {t}\r\n"))
            .unwrap_or_default();
        let req = format!(
            "POST /mcp HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\n{auth}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        s.write_all(req.as_bytes()).await.unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).await.unwrap();
        let status = out.split_whitespace().nth(1).unwrap().parse().unwrap();
        (
            status,
            out.split("\r\n\r\n").nth(1).unwrap_or_default().to_string(),
        )
    }

    fn rpc(id: u32, method: &str, params: Value) -> Value {
        json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
    }

    async fn call(tasks: &Tasks, token: &str, id: u32, tool: &str, args: Value) -> (bool, String) {
        let (st, body) = post(
            tasks.url(),
            Some(token),
            &rpc(id, "tools/call", json!({ "name": tool, "arguments": args })),
        )
        .await;
        assert_eq!(st, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        (
            v["result"]["isError"].as_bool().unwrap(),
            v["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .to_string(),
        )
    }

    /// An embedder's tools: `echo_owner` says who called it; `task_list` tries to shadow a built-in one.
    struct Echo;

    impl Tools for Echo {
        fn list(&self) -> Vec<Value> {
            vec![json!({ "name": "echo_owner", "inputSchema": { "type": "object" } })]
        }

        fn call<'a>(
            &'a self,
            scope: &'a Scope,
            name: &'a str,
            args: &'a Value,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<Result<String, String>>> + Send + 'a>> {
            Box::pin(async move {
                match name {
                    "echo_owner" if args["fail"] == true => Some(Err("asked to fail".into())),
                    "echo_owner" => Some(Ok(format!("owner {}", scope.owner.as_deref().unwrap_or("-")))),
                    "task_list" => Some(Ok("shadowed".into())),
                    _ => None,
                }
            })
        }
    }

    #[tokio::test]
    async fn serves_the_embedders_tools_under_its_name() {
        let dir = tmp("mcp-extra");
        let tasks = tasks_in(&dir).await;
        tasks.set_server_name("host");
        tasks.add_tools(Arc::new(Echo));
        let grant = tasks.grant(Scope { owner: Some("run-1".into()), project: None });

        let (_, body) = post(tasks.url(), Some(&grant.token), &rpc(1, "initialize", json!({}))).await;
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["result"]["serverInfo"]["name"], "host");

        let (_, body) = post(tasks.url(), Some(&grant.token), &rpc(2, "tools/list", json!({}))).await;
        let v: Value = serde_json::from_str(&body).unwrap();
        let names: Vec<_> = v["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!((names.first(), names.last()), (Some(&"task_start"), Some(&"echo_owner")));

        // The call gets the grant's scope; errors come back as tool errors.
        assert_eq!(call(&tasks, &grant.token, 3, "echo_owner", json!({})).await, (false, "owner run-1".into()));
        assert_eq!(call(&tasks, &grant.token, 4, "echo_owner", json!({ "fail": true })).await, (true, "asked to fail".into()));
        // bach-tasks' own tools come first; unknown names are still errors.
        assert_eq!(call(&tasks, &grant.token, 5, "task_list", json!({})).await, (false, "No background tasks.".into()));
        assert_eq!(call(&tasks, &grant.token, 6, "nope", json!({})).await, (true, "Unknown tool `nope`".into()));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn speaks_mcp_and_manages_tasks_for_its_own_project() {
        let dir = tmp("mcp");
        let proj_a = dir.join("a");
        let proj_b = dir.join("b");
        std::fs::create_dir_all(&proj_a).unwrap();
        std::fs::create_dir_all(&proj_b).unwrap();
        let tasks = tasks_in(&dir).await;
        let grant_a = tasks.grant(Scope {
            owner: Some("run-a".into()),
            project: Some(proj_a.to_string_lossy().into()),
        });
        let grant_b = tasks.grant(Scope {
            owner: Some("run-b".into()),
            project: Some(proj_b.to_string_lossy().into()),
        });
        let (ta, tb) = (grant_a.token.clone(), grant_b.token.clone());

        assert_eq!(grant_a.url, tasks.url());

        // No token, or a wrong one: refused.
        assert_eq!(
            post(tasks.url(), None, &rpc(1, "ping", json!({}))).await.0,
            401
        );
        assert_eq!(
            post(tasks.url(), Some("nope"), &rpc(1, "ping", json!({})))
                .await
                .0,
            401
        );

        let (st, body) = post(
            tasks.url(),
            Some(&ta),
            &rpc(1, "initialize", json!({ "protocolVersion": "2025-06-18" })),
        )
        .await;
        assert_eq!(st, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(
            (
                v["result"]["serverInfo"]["name"].as_str(),
                v["result"]["protocolVersion"].as_str()
            ),
            (Some("bach-tasks"), Some("2025-06-18"))
        );
        let instructions = v["result"]["instructions"].as_str().unwrap();
        assert!(instructions.contains("task_start") && instructions.contains("run_in_background"));
        let note = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        assert_eq!(post(tasks.url(), Some(&ta), &note).await.0, 202);
        let (_, body) = post(tasks.url(), Some(&ta), &rpc(2, "tools/list", json!({}))).await;
        let v: Value = serde_json::from_str(&body).unwrap();
        let names: Vec<_> = v["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            [
                "task_start",
                "compose_start",
                "task_process",
                "task_list",
                "task_logs",
                "task_stop",
                "port_info",
                "http_check"
            ]
        );

        // Start: defaults to the run's project, waits briefly, reports state and first output.
        let (err, text) = call(
            &tasks,
            &ta,
            3,
            "task_start",
            json!({ "command": "echo up; sleep 4706", "name": "web" }),
        )
        .await;
        assert!(!err, "{text}");
        assert!(
            text.contains("\"web\": running")
                && text.contains("Output so far:")
                && text.contains("up"),
            "{text}"
        );
        let id = tasks.list(None)[0].task.id.clone();
        let t = tasks.get(&id).unwrap();
        assert_eq!(
            (t.cwd.as_str(), t.owner.as_deref()),
            (proj_a.to_str().unwrap(), Some("run-a"))
        );

        // List/logs/stop see it from project A, but project B's run does not.
        let (_, list) = call(&tasks, &ta, 4, "task_list", json!({})).await;
        assert!(list.contains(&id) && list.contains("running"), "{list}");
        assert_eq!(
            call(&tasks, &tb, 5, "task_list", json!({})).await.1,
            "No background tasks."
        );
        for tool in ["task_logs", "task_stop"] {
            let (err, text) = call(&tasks, &tb, 6, tool, json!({ "id": id })).await;
            assert!(err && text.contains("No task"), "{tool}: {text}");
        }
        assert_eq!(
            status(&tasks, &id),
            TaskStatus::Running,
            "project B could not stop it"
        );

        let (_, logs) = call(&tasks, &ta, 7, "task_logs", json!({ "id": id, "lines": 5 })).await;
        assert!(logs.contains("up"), "{logs}");
        let (err, text) = call(&tasks, &ta, 8, "task_stop", json!({ "id": id })).await;
        assert!(!err && text.contains("stopped"), "{text}");
        until("stopped", || procs_matching("sleep 4706").is_empty()).await;

        // A bad call is a tool error the agent can read, not a transport failure.
        assert!(call(&tasks, &ta, 9, "task_start", json!({})).await.0);
        let v: Value = serde_json::from_str(
            &post(tasks.url(), Some(&ta), &rpc(10, "nope", json!({})))
                .await
                .1,
        )
        .unwrap();
        assert_eq!(v["error"]["code"], -32601);

        // Once the run is over its token stops working.
        assert_eq!(tasks.active_grants(), 2);
        drop(grant_a);
        assert_eq!(tasks.active_grants(), 1);
        assert_eq!(
            post(tasks.url(), Some(&ta), &rpc(11, "ping", json!({})))
                .await
                .0,
            401
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    // ----- diagnosing conflicts, several ports, HTTP readiness ------------------------------

    /// A process that is not a background task, listening on `port`.
    struct Foreign(std::process::Child);
    impl Foreign {
        async fn listening_on(port: u16, dir: &Path) -> Foreign {
            let child = std::process::Command::new("python3")
                .args([
                    "-m",
                    "http.server",
                    &port.to_string(),
                    "--bind",
                    "127.0.0.1",
                ])
                .current_dir(dir)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            until("foreign listener", || {
                std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
            })
            .await;
            Foreign(child)
        }
        fn pid(&self) -> u32 {
            self.0.id()
        }
    }
    impl Drop for Foreign {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    pub(super) fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    pub(super) fn run_in(dir: &Path) -> Scope {
        Scope {
            owner: Some("r".into()),
            project: Some(dir.to_string_lossy().into()),
        }
    }

    #[tokio::test]
    async fn explains_who_holds_a_port_a_task_could_not_get() {
        let dir = tmp("conflict");
        let tasks = tasks_in(&dir).await;
        let port = free_port();
        let squatter = Foreign::listening_on(port, &dir).await;

        // 1) The log names the port (as vite/node do): the holder is found and described.
        let t = tasks
            .start_task(req(
                &format!("echo 'Error: Port {port} is already in use'; exit 1"),
                &dir,
            ))
            .unwrap();
        until("failed", || status(&tasks, &t.id) == TaskStatus::Failed).await;
        let view = tasks.view(&t.id).unwrap();
        let text = view.problems.join("\n");
        assert!(
            text.contains(&format!(
                "Port {port} is already in use by pid {}",
                squatter.pid()
            )),
            "{text}"
        );
        assert!(
            text.contains("not a background task") && text.contains(dir.to_str().unwrap()),
            "{text}"
        );
        assert!(
            Tasks::describe(&view).contains("Problem: Port"),
            "{}",
            Tasks::describe(&view)
        );

        // 2) The holder can be one of our own tasks, and is named as such.
        let other = tasks
            .start_task(req(
                &format!("python3 -m http.server {} --bind 127.0.0.1", free_port()),
                &dir,
            ))
            .unwrap();
        let ported = tasks.start_task(req("sleep 60", &dir)).unwrap();
        let _ = (other, ported);

        // 3) port_info answers the question directly, for one port and for a port nobody has.
        let report = tasks.port_report(&[port, free_port()]);
        assert!(
            report.contains(&format!("Port {port}: pid {}", squatter.pid())),
            "{report}"
        );
        assert!(
            report.contains("not a background task") && report.contains("up "),
            "{report}"
        );
        assert!(report.contains("nothing is listening"), "{report}");
        for t in tasks.list(None) {
            let _ = tasks.stop_task(&t.task.id).await;
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Supervisors like process-compose (or a shell with job control) put each child in a process
    /// group of its own. The child is still the task's: its ports, its processes, and stopped with it.
    #[tokio::test]
    async fn children_in_their_own_process_group_belong_to_the_task() {
        let dir = tmp("pgroup");
        let tasks = tasks_in(&dir).await;
        let port = free_port();
        let t = tasks
            .start_task(StartTask {
                ports: vec![port],
                // `set -m`: background jobs get their own process group.
                ..req(
                    &format!("set -m; python3 -m http.server {port} --bind 127.0.0.1 & wait"),
                    &dir,
                )
            })
            .unwrap();
        tasks
            .wait_ready(&t.id, &[port], None, Duration::from_secs(10))
            .await;
        // The python process, not the `sh -c` wrappers whose command lines mention it.
        let server = || {
            procs_matching(&format!("http.server {port}"))
                .into_iter()
                .filter(|pid| {
                    sys::argv(*pid)
                        .first()
                        .is_some_and(|a| a.rsplit('/').next().unwrap().starts_with("python3"))
                })
                .collect::<Vec<u32>>()
        };
        let server_pids = server();
        assert_eq!(server_pids.len(), 1, "{server_pids:?}");
        assert_ne!(
            proc_group(server_pids[0]),
            t.pid,
            "the server moved to its own group (else this test proves nothing)"
        );

        let view = tasks.view(&t.id).unwrap();
        assert_eq!(view.up_ports, vec![port], "{view:?}");
        assert!(
            view.missing_ports.is_empty() && view.problems.is_empty(),
            "{view:?}"
        );
        assert!(
            view.processes.iter().any(|p| p.starts_with("python3")),
            "{view:?}"
        );
        let report = tasks.port_report(&[port]);
        assert!(report.contains(&format!("background task {}", t.id)), "{report}");

        tasks.stop_task(&t.id).await.unwrap();
        until("server gone", || server().is_empty()).await;
        let _ = std::fs::remove_dir_all(dir);
    }

    fn proc_group(pid: u32) -> u32 {
        // SAFETY: plain getpgid(2).
        unsafe { libc::getpgid(pid as i32) as u32 }
    }

    #[tokio::test]
    async fn a_port_taken_after_a_task_ended_is_not_why_it_failed() {
        let dir = tmp("later");
        let tasks = tasks_in(&dir).await;
        let port = free_port();
        let t = tasks
            .start_task(StartTask {
                ports: vec![port],
                ..req("exit 1", &dir)
            })
            .unwrap();
        until("failed", || status(&tasks, &t.id) == TaskStatus::Failed).await;
        // Its successor (here: anything) takes the port a while later.
        tokio::time::sleep(Duration::from_millis(2100)).await;
        let _later = Foreign::listening_on(port, &dir).await;
        let view = tasks.view(&t.id).unwrap();
        assert!(
            !view.problems.iter().any(|p| p.contains("already in use")),
            "{view:?}"
        );
        assert!(
            view.problems.iter().any(|p| p.contains(&format!("Expected port {port} is not listening"))),
            "{view:?}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn port_info_recognises_its_own_tasks() {
        let dir = tmp("portinfo");
        let tasks = tasks_in(&dir).await;
        let port = free_port();
        let t = tasks
            .start_task(req(
                &format!("python3 -m http.server {port} --bind 127.0.0.1"),
                &dir,
            ))
            .unwrap();
        tasks
            .wait_ready(&t.id, &[port], None, Duration::from_secs(10))
            .await;
        let report = tasks.port_report(&[port]);
        assert!(report.contains(&format!("background task {}", t.id)), "{report}");
        assert!(!report.contains("not a background task"), "{report}");

        // A second task that wants the same port is told exactly which task has it.
        let clash = tasks
            .start_task(StartTask {
                ports: vec![port],
                ..req(
                    &format!("python3 -m http.server {port} --bind 127.0.0.1"),
                    &dir,
                )
            })
            .unwrap();
        until("clash exits", || {
            status(&tasks, &clash.id) != TaskStatus::Running
        })
        .await;
        let view = tasks
            .list_settled(None, 0)
            .into_iter()
            .find(|v| v.task.id == clash.id)
            .unwrap();
        assert!(
            view.problems.iter().any(|p| p.contains(&format!(
                "Port {port} is already in use by background task {}",
                t.id
            ))),
            "{:?}",
            view.problems
        );
        tasks.stop_task(&t.id).await.unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn task_start_reports_which_expected_ports_came_up_and_gives_up_early() {
        let dir = tmp("expected");
        let tasks = tasks_in(&dir).await;
        tasks.set_settle_ms(3000); // a short grace period so the test can see both sides of it
        let (good, taken) = (free_port(), free_port());
        let squatter = Foreign::listening_on(taken, &dir).await;
        let a = format!("python3 -m http.server {good} --bind 127.0.0.1");
        let b = format!("python3 -m http.server {taken} --bind 127.0.0.1");

        // Two services, one of which cannot get its port: like `just dev` with a stale vite.
        let started = std::time::Instant::now();
        let (err, text) = {
            let run = run_in(&dir);
            let r = tasks
                .call(&run, "task_start", &json!({ "command": format!("{a} & {b} & wait"), "name": "stack", "ports": [good, taken] }))
                .await;
            (r.is_err(), r.unwrap_or_else(|e| e))
        };
        let waited = started.elapsed();
        assert!(!err, "{text}");
        assert!(
            waited < Duration::from_secs(12),
            "must not sit out the full 30s timeout: {waited:?}"
        );
        let expected_line = text
            .lines()
            .find(|l| l.contains("Expected ports:"))
            .expect("an Expected ports line");
        assert!(
            expected_line.contains(&format!("{good} up"))
                && expected_line.contains(&format!("{taken} NOT listening")),
            "{text}"
        );
        assert!(
            text.contains(&format!(
                "Problem: Port {taken} is already in use by pid {}",
                squatter.pid()
            )),
            "{text}"
        );
        assert!(
            text.contains("Processes:") && text.contains("python3"),
            "which processes are alive: {text}"
        );

        // The same picture is available later, from task_list.
        let (_, list) = (
            0,
            tasks
                .call(&run_in(&dir), "task_list", &json!({}))
                .await
                .unwrap(),
        );
        // (still inside the grace period: not yet called a failure, and not claimed to be up)
        assert!(list.contains(&format!("{taken} not up yet")), "{list}");
        assert!(!list.contains(&format!("{taken} up")), "{list}");
        tokio::time::sleep(Duration::from_millis(3200)).await;
        let later = tasks
            .call(&run_in(&dir), "task_list", &json!({}))
            .await
            .unwrap();
        assert!(later.contains(&format!("{taken} NOT listening")), "{later}");
        assert!(later.contains("Problem: Port"), "{later}");
        // ...but a task that is simply still starting is not reported as broken.
        let slow = tasks
            .start_task(StartTask {
                ports: vec![free_port()],
                ..req("sleep 30", &dir)
            })
            .unwrap();
        let view = tasks.view(&slow.id).unwrap();
        assert!(
            view.problems.is_empty() && view.missing_ports.is_empty(),
            "{:?}",
            view.problems
        );

        for t in tasks.list(None) {
            tasks.stop_task(&t.task.id).await.unwrap();
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn waits_for_all_expected_ports_and_an_http_answer() {
        let dir = tmp("ready");
        std::fs::write(dir.join("index.html"), "hi").unwrap();
        let tasks = tasks_in(&dir).await;
        let (p1, p2) = (free_port(), free_port());
        let cmd = format!("python3 -m http.server {p1} --bind 127.0.0.1 & python3 -m http.server {p2} --bind 127.0.0.1 & wait");
        let run = run_in(&dir);
        let text = tasks
            .call(&run, "task_start", &json!({ "command": cmd, "ports": [p1, p2], "ready_url": format!("http://localhost:{p2}/index.html") }))
            .await
            .unwrap();
        let expected_line = text
            .lines()
            .find(|l| l.contains("Expected ports:"))
            .expect("an Expected ports line");
        assert!(
            expected_line.contains(&format!("{p1} up"))
                && expected_line.contains(&format!("{p2} up")),
            "{text}"
        );
        assert!(
            text.contains(&format!(
                "HTTP check: GET http://localhost:{p2}/index.html -> 200"
            )),
            "{text}"
        );

        // A required status that is not what the server returns is reported, not hidden.
        let text2 = tasks
            .call(
                &run,
                "task_start",
                &json!({ "command": "sleep 30", "ready_url": format!("http://localhost:{p1}/nope.html"), "ready_status": 200, "ports": [], "timeout_seconds": 2 }),
            )
            .await
            .unwrap();
        assert!(
            text2.contains("HTTP check: GET") && text2.contains("-> 404"),
            "{text2}"
        );

        // Only local URLs are ever requested, and a bad one is refused before anything is launched.
        let before = tasks.list(None).len();
        let started = std::time::Instant::now();
        let bad = tasks
            .call(
                &run,
                "task_start",
                &json!({ "command": "sleep 30", "ready_url": "http://example.com/" }),
            )
            .await
            .unwrap_err();
        assert!(
            bad.contains("ready_url") && bad.contains("Only localhost"),
            "{bad}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "must fail at once, not time out"
        );
        assert_eq!(tasks.list(None).len(), before, "no task was started");
        let check = tasks
            .call(
                &run,
                "http_check",
                &json!({ "url": format!("http://127.0.0.1:{p1}/index.html") }),
            )
            .await
            .unwrap();
        assert!(check.contains("-> 200"), "{check}");
        let closed = tasks
            .call(
                &run,
                "http_check",
                &json!({ "url": format!("http://127.0.0.1:{}/", free_port()) }),
            )
            .await
            .unwrap();
        assert!(closed.contains("did not answer"), "{closed}");
        for t in tasks.list(None) {
            tasks.stop_task(&t.task.id).await.unwrap();
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn serves_a_task_log_in_chunks_that_follow_it() {
        let dir = tmp("chunk");
        let tasks = tasks_in(&dir).await;
        let t = tasks
            .start_task(req("echo first; sleep 1; echo second; sleep 30", &dir))
            .unwrap();
        // (The log's first line echoes the command, so match whole lines.)
        let has_line = |c: &LogChunk, l: &str| c.text.lines().any(|x| x == l);
        until("first line", || {
            tasks
                .log_chunk(&t.id, None, None, 4096)
                .is_ok_and(|c| has_line(&c, "first"))
        })
        .await;
        let c1 = tasks.log_chunk(&t.id, None, None, 4096).unwrap();
        until("second line", || {
            tasks
                .log_chunk(&t.id, None, Some(c1.next), 4096)
                .is_ok_and(|c| has_line(&c, "second"))
        })
        .await;
        let c2 = tasks.log_chunk(&t.id, None, Some(c1.next), 4096).unwrap();
        assert!(
            has_line(&c2, "second") && !has_line(&c2, "first"),
            "a continuation, not a re-read: {c2:?}"
        );
        assert_eq!(c2.offset, c1.next);
        assert!(tasks.log_chunk("nope", None, None, 100).is_err());
        tasks.stop_task(&t.id).await.unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn tells_subscribers_what_changed() {
        let dir = tmp("events");
        let tasks = tasks_in(&dir).await;
        let mut events = tasks.subscribe();
        async fn next(events: &mut tokio::sync::broadcast::Receiver<TaskEvent>) -> TaskEvent {
            tokio::time::timeout(Duration::from_secs(5), events.recv())
                .await
                .expect("an event")
                .unwrap()
        }

        let t = tasks.start_task(req("sleep 4707", &dir)).unwrap();
        match next(&mut events).await {
            TaskEvent::Changed { task } => {
                assert_eq!((task.task.id.as_str(), task.task.status), (t.id.as_str(), TaskStatus::Running))
            }
            other => panic!("{other:?}"),
        }
        tasks.stop_task(&t.id).await.unwrap();
        // Its view may change while it runs (its processes, say); then comes the stop.
        loop {
            match next(&mut events).await {
                TaskEvent::Changed { task } if task.task.status == TaskStatus::Running => {}
                TaskEvent::Changed { task } => {
                    assert_eq!(task.task.status, TaskStatus::Stopped);
                    break;
                }
                other => panic!("{other:?}"),
            }
        }
        // Only changes are sent: nothing more until something else happens.
        assert!(
            tokio::time::timeout(Duration::from_millis(300), events.recv())
                .await
                .is_err()
        );
        tasks.remove_task(&t.id).unwrap();
        assert!(matches!(next(&mut events).await, TaskEvent::Removed { id } if id == t.id));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn remembers_whether_a_task_is_interactive() {
        let dir = tmp("interactive");
        let tasks = tasks_in(&dir).await;
        let run = run_in(&dir);
        let text = tasks
            .call(&run, "task_start", &json!({ "command": "sleep 4706", "name": "tests", "interactive": false }))
            .await
            .unwrap();
        assert!(text.contains("not interactive"), "{text}");
        let quiet = text.split_whitespace().nth(1).unwrap().to_string();
        let text = tasks.call(&run, "task_start", &json!({ "command": "sleep 4707", "name": "dev" })).await.unwrap();
        assert!(!text.contains("interactive"), "{text}");
        let dev = text.split_whitespace().nth(1).unwrap().to_string();
        assert!(!tasks.get(&quiet).unwrap().interactive);
        assert!(tasks.get(&dev).unwrap().interactive, "interactive unless said otherwise");

        drop(tasks);
        let after = tasks_in(&dir).await;
        assert!(!after.get(&quiet).unwrap().interactive, "saved with the task");
        after.stop_task(&quiet).await.unwrap();
        after.stop_task(&dev).await.unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn adopts_tasks_recorded_by_an_older_version() {
        let dir = tmp("import");
        let tasks = tasks_in(&dir).await;
        let log = dir.join("old.log");
        std::fs::write(&log, "$ make serve\nserving\n").unwrap();
        // How tasks were saved in Bach's own database, owned by a `runId`.
        let old = json!({
            "id": "old1", "name": "serve", "command": "make serve", "cwd": dir, "project": dir,
            "runId": "run-9", "pid": 999_999_999u32, "startTicks": null, "startedAt": 1,
            "endedAt": null, "status": "running", "exitCode": null, "logPath": log,
        });
        let task = crate::parse_task(old).expect("old tasks still parse");
        assert_eq!(task.owner.as_deref(), Some("run-9"));
        assert!(task.interactive, "tasks from before the flag are the user's");
        assert_eq!(tasks.import(vec![task.clone()]), 1);
        assert_eq!(tasks.import(vec![task]), 0, "already known");
        // Its process is long gone, which the import notices straight away.
        assert_eq!(status(&tasks, "old1"), TaskStatus::Lost);
        assert!(tasks.logs("old1", 5, None).unwrap().contains("serving"));

        // And it's in bach-tasks' own database from now on.
        drop(tasks);
        assert_eq!(status(&tasks_in(&dir).await, "old1"), TaskStatus::Lost);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn unknown_tasks_are_not_found() {
        let dir = tmp("errors");
        let tasks = tasks_in(&dir).await;
        assert!(matches!(tasks.stop_task("nope").await, Err(Error::NotFound(_))));
        assert!(matches!(tasks.remove_task("nope"), Err(Error::NotFound(_))));
        assert!(matches!(tasks.logs("nope", 1, None), Err(Error::NotFound(_))));
        assert!(matches!(
            tasks.start_task(req("  ", &dir)),
            Err(Error::Invalid(_))
        ));
        let t = tasks.start_task(req("sleep 4708", &dir)).unwrap();
        assert!(matches!(tasks.remove_task(&t.id), Err(Error::Invalid(_))));
        tasks.stop_task(&t.id).await.unwrap();
        let _ = std::fs::remove_dir_all(dir);
    }
