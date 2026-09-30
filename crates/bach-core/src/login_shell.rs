//! The PATH the user's shell sets up. A process not started from a terminal gets a bare one: an
//! app opened from the Finder, or bach-server started by `ssh <host> bach-server attach` (which
//! runs no profile). Without Homebrew, Nix or ~/.local/bin on it, `claude`, `codex`, `git` and
//! whatever `ssh` needs wouldn't be found.
use std::{process::Command, sync::mpsc, time::Duration};

const MARK: &str = "__bach_path__";

/// The PATH of an interactive login shell (so both `.zprofile` and `.zshrc`, say, have run), or
/// `None` if it can't be had within a few seconds.
pub fn path() -> Option<String> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let out = Command::new(shell)
            .args(["-l", "-i", "-c", &format!("printf '{MARK}%s{MARK}' \"$PATH\"")])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output();
        let _ = tx.send(out);
    });
    // A profile that hangs mustn't hang whoever asked.
    let out = rx.recv_timeout(Duration::from_secs(5)).ok()?.ok()?;
    // An interactive profile may print things of its own around it.
    let path = String::from_utf8_lossy(&out.stdout).split(MARK).nth(1)?.trim().to_string();
    (!path.is_empty()).then_some(path)
}

#[cfg(test)]
mod tests {
    #[test]
    fn finds_a_path() {
        let path = super::path().expect("a login shell's PATH");
        assert!(path.contains('/'), "{path}");
    }
}
