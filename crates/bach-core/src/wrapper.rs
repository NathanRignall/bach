//! A command the agent CLIs are started through on this machine, such as a sandbox: with
//! `sandbox`, Claude Code runs as `sandbox claude …`. Set by a client (the desktop app's SSH
//! connection settings), or with `BACH_AGENT_WRAPPER` when the server starts. Only with the
//! `agent-wrapper` feature; without it agents always start directly.
use std::sync::{OnceLock, RwLock};

fn current() -> &'static RwLock<Vec<String>> {
    static WRAPPER: OnceLock<RwLock<Vec<String>>> = OnceLock::new();
    WRAPPER.get_or_init(|| {
        let words = std::env::var("BACH_AGENT_WRAPPER")
            .ok()
            .filter(|_| cfg!(feature = "agent-wrapper"))
            .and_then(|w| parse(&w).ok())
            .unwrap_or_default();
        RwLock::new(words)
    })
}

/// A wrapper as typed in a shell, as words (none for `""`).
pub fn parse(wrapper: &str) -> Result<Vec<String>, String> {
    shlex::split(wrapper)
        .ok_or_else(|| format!("Couldn't read `{wrapper}` as a command; check its quotes."))
}

/// Starts agents through `wrapper` from now on (`""` for directly).
pub fn set(wrapper: &str) -> Result<(), String> {
    let words = parse(wrapper)?;
    if !words.is_empty() && !cfg!(feature = "agent-wrapper") {
        return Err("This bach-server was built without `agent-wrapper`; it starts agents directly.".into());
    }
    *current().write().unwrap() = words;
    Ok(())
}

fn command_with(wrapper: &[String], binary: &str) -> Vec<String> {
    wrapper.iter().cloned().chain([binary.to_string()]).collect()
}

/// The program and arguments that start `binary`: `["claude"]`, or `["sandbox", "claude"]`.
pub fn command(binary: &str) -> Vec<String> {
    command_with(&current().read().unwrap(), binary)
}

/// Whether agents start through a wrapper, which then provides their sandbox.
pub fn active() -> bool {
    !current().read().unwrap().is_empty()
}

/// [`command`] for a shell command line.
pub fn shell_command(binary: &str) -> String {
    shlex::try_join(command(binary).iter().map(String::as_str)).expect("no NUL bytes in a command")
}

/// Whether `binary` can be started: it and the wrapper are on the PATH.
pub fn installed(binary: &str) -> bool {
    let command = command(binary);
    which::which(&command[0]).is_ok() && which::which(binary).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_the_agent_with_the_wrapper() {
        assert_eq!(command_with(&[], "claude"), ["claude"]);
        let w = parse("sandbox --net 'strict mode'").unwrap();
        assert_eq!(command_with(&w, "codex"), ["sandbox", "--net", "strict mode", "codex"]);
        assert!(parse("  ").unwrap().is_empty());
        assert!(parse("sandbox 'oops").unwrap_err().contains("quotes"));
    }

    #[cfg(not(feature = "agent-wrapper"))]
    #[test]
    fn only_directly_without_the_feature() {
        assert!(set("sandbox").unwrap_err().contains("agent-wrapper"));
        assert!(set("").is_ok() && !active());
    }
}
