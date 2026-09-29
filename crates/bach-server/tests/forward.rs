//! Port forwarding over the app's own SSH connection, against a real sshd: a private one run as
//! this user on a free port, with throwaway keys. Skipped where there's no sshd to run.
use bach_client::{
    forward::{master_options, prepare_control_dir, Forward, Forwards},
    Remote, Status,
};
use serde_json::json;
use std::{
    collections::BTreeSet,
    io::{Read, Write},
    net::TcpStream,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::Duration,
};

struct Sshd {
    dir: PathBuf,
    child: Child,
}

impl Drop for Sshd {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

/// An sshd for one connection (`-d`: it runs unprivileged that way), and an ssh config with a
/// `testhost` entry that logs into it.
fn sshd() -> Option<Sshd> {
    let sshd = ["/run/current-system/sw/bin/sshd", "/usr/sbin/sshd"]
        .into_iter()
        .find(|p| Path::new(p).exists())?;
    let dir = std::env::temp_dir().join(format!("bach-sshd-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for key in ["host_key", "client_key"] {
        let ok = Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-f"])
            .arg(dir.join(key))
            .status()
            .ok()?
            .success();
        assert!(ok);
    }
    std::fs::copy(dir.join("client_key.pub"), dir.join("authorized_keys")).unwrap();
    let port = free_port();
    let d = dir.display();
    std::fs::write(
        dir.join("sshd_config"),
        format!(
            "Port {port}\nListenAddress 127.0.0.1\nHostKey {d}/host_key\nPidFile {d}/sshd.pid\n\
             AuthorizedKeysFile {d}/authorized_keys\nPasswordAuthentication no\n\
             KbdInteractiveAuthentication no\nStrictModes no\nUsePAM no\nAllowTcpForwarding yes\n"
        ),
    )
    .unwrap();
    std::fs::write(
        dir.join("ssh_config"),
        format!(
            "Host testhost\n  HostName 127.0.0.1\n  Port {port}\n  User {}\n  IdentityFile {d}/client_key\n  \
             IdentitiesOnly yes\n  UserKnownHostsFile {d}/known_hosts\n  StrictHostKeyChecking accept-new\n",
            std::env::var("USER").unwrap()
        ),
    )
    .unwrap();
    let child = Command::new(sshd)
        .arg("-d")
        .arg("-f")
        .arg(dir.join("sshd_config"))
        .stdout(Stdio::null())
        .stderr(std::fs::File::create(dir.join("sshd.log")).unwrap())
        .spawn()
        .ok()?;
    // Not by connecting: in `-d` mode sshd serves exactly one connection.
    for _ in 0..50 {
        let log = std::fs::read_to_string(dir.join("sshd.log")).unwrap_or_default();
        if log.contains("Server listening") {
            return Some(Sshd { dir, child });
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    None
}

fn http_get(port: u16) -> Result<String, String> {
    let mut s = TcpStream::connect(("127.0.0.1", port)).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    s.write_all(b"GET /hello.txt HTTP/1.0\r\nHost: x\r\n\r\n")
        .map_err(|e| e.to_string())?;
    let mut out = String::new();
    s.read_to_string(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

async fn until(what: &str, mut cond: impl FnMut() -> bool) {
    for _ in 0..200 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for: {what}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forwards_ports_over_the_apps_ssh_connection() {
    let Some(sshd) = sshd() else {
        eprintln!("no sshd to test with; skipping");
        return;
    };
    let config = sshd.dir.join("ssh_config");
    let control = sshd.dir.join("cm-%C");
    prepare_control_dir(&control).unwrap();
    let data = sshd.dir.join("data");
    let www = sshd.dir.join("www");
    std::fs::create_dir_all(&www).unwrap();
    std::fs::write(www.join("hello.txt"), "hello through the tunnel").unwrap();

    // What the app runs, with a config that points `testhost` at our sshd.
    let ssh_opts = vec!["-F".to_string(), config.display().to_string()];
    let remote_command = format!(
        "BACH_DB='{}' BACH_PORT={} '{}' attach",
        data.join("bach.db").display(),
        free_port(),
        env!("CARGO_BIN_EXE_bach-server")
    );
    let command: Vec<String> = ["ssh".to_string()]
        .into_iter()
        .chain(ssh_opts.clone())
        .chain(master_options(&control))
        .chain(["-T", "-o", "BatchMode=yes", "testhost", "--"].map(String::from))
        .chain([remote_command])
        .collect();
    let remote = Remote::connect("testhost", command, |_| {}, |_| {});
    let agents = remote.call("list_agents", json!({})).await;
    assert!(agents.is_ok(), "{agents:?}\n{}", std::fs::read_to_string(sshd.dir.join("sshd.log")).unwrap_or_default());
    assert!(matches!(remote.status(), Status::Connected { .. }));

    // A web server on the "remote", started as a background task.
    let port = free_port();
    let task = remote
        .call(
            "start_task",
            json!({ "command": format!("python3 -m http.server {port} --bind 127.0.0.1"), "cwd": www, "name": "www" }),
        )
        .await
        .unwrap();
    until("the web server", || http_get(port).is_ok()).await;

    let changes = std::sync::Arc::new(std::sync::Mutex::new(0));
    let forwards = Forwards::new("ssh", ssh_opts, "testhost", control.clone(), {
        let changes = changes.clone();
        move |_| *changes.lock().unwrap() += 1
    });

    // By hand. The port is taken here (remote and local are the same machine in this test), so
    // another local port is used.
    let f = forwards.add(port, false).await.unwrap();
    assert_ne!(f.local, port);
    assert!(!f.auto);
    let page = http_get(f.local).unwrap();
    assert!(page.contains("hello through the tunnel"), "{page}");
    assert_eq!(forwards.add(port, true).await.unwrap(), f, "already forwarded");

    // Automatic forwards leave ones added by hand alone.
    assert!(forwards.set_auto(&BTreeSet::new()).await.is_empty());
    assert_eq!(forwards.list().await, vec![f.clone()]);
    forwards.remove(port).await.unwrap();
    until("the forward to close", || http_get(f.local).is_err()).await;
    assert!(forwards.list().await.is_empty());

    // Automatic ones follow the set.
    assert!(forwards.set_auto(&BTreeSet::from([port])).await.is_empty());
    let auto = forwards.list().await;
    assert!(matches!(auto.as_slice(), [Forward { auto: true, .. }]), "{auto:?}");
    assert!(http_get(auto[0].local).unwrap().contains("hello through the tunnel"));
    forwards.set_auto(&BTreeSet::new()).await;
    assert!(forwards.list().await.is_empty());
    assert!(*changes.lock().unwrap() >= 4);

    remote
        .call("stop_task", json!({ "taskId": task["id"] }))
        .await
        .unwrap();
    drop(remote);
    // The server `attach` started runs on; stop it.
    let want = format!("BACH_DB={}", data.join("bach.db").display());
    for pid in std::fs::read_dir("/proc")
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().to_str()?.parse::<i32>().ok())
    {
        let env = std::fs::read(format!("/proc/{pid}/environ")).unwrap_or_default();
        if env.split(|b| *b == 0).any(|v| v == want.as_bytes()) {
            unsafe { kill(pid, 15) };
        }
    }
}

extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}
