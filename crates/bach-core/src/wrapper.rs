//! A command the agent CLIs are started through on this machine, such as a sandbox: with
//! `sandbox`, Claude Code runs as `sandbox claude …`. Read once, when the server starts, from
//! `BACH_AGENT_WRAPPER` (and `BACH_AGENT_WRAPPER_CODEX_SANDBOX`), and fixed from then on: nothing
//! a client sends can take it away. Only with the `agent-wrapper` feature; without it a server
//! asked for a wrapper refuses to start rather than start agents directly.
use bach_protocol::AgentWrapper;
use std::sync::OnceLock;

struct Current {
    wrapper: AgentWrapper,
    words: Vec<String>,
}

static CURRENT: OnceLock<Current> = OnceLock::new();

fn load(command: &str, codex_sandbox: bool) -> Result<Current, String> {
    let words = parse(command)?;
    if !words.is_empty() && !cfg!(feature = "agent-wrapper") {
        return Err(format!(
            "BACH_AGENT_WRAPPER is `{command}`, but this bach-server was built without `agent-wrapper`."
        ));
    }
    let wrapper = AgentWrapper { command: command.trim().to_string(), codex_sandbox };
    Ok(Current { wrapper, words })
}

fn from_env() -> Result<Current, String> {
    let command = std::env::var("BACH_AGENT_WRAPPER").unwrap_or_default();
    let codex_sandbox = std::env::var("BACH_AGENT_WRAPPER_CODEX_SANDBOX").is_ok_and(|v| v == "1");
    load(&command, codex_sandbox)
}

/// Reads the wrapper from the environment, failing if it can't be used. Called as the backend
/// starts, so a bad one stops it there.
pub fn init() -> Result<(), String> {
    if CURRENT.get().is_none() {
        let _ = CURRENT.set(from_env()?);
    }
    Ok(())
}

fn current() -> &'static Current {
    CURRENT.get_or_init(|| from_env().unwrap_or_else(|e| panic!("{e}")))
}

/// How agents are started, for clients to check against what they asked for.
pub fn get() -> &'static AgentWrapper {
    &current().wrapper
}

/// A wrapper as typed in a shell, as words (none for `""`).
pub fn parse(wrapper: &str) -> Result<Vec<String>, String> {
    shlex::split(wrapper)
        .ok_or_else(|| format!("Couldn't read `{wrapper}` as a command; check its quotes."))
}

fn command_with(wrapper: &[String], binary: &str) -> Vec<String> {
    wrapper.iter().cloned().chain([binary.to_string()]).collect()
}

/// The program and arguments that start `binary`: `["claude"]`, or `["sandbox", "claude"]`.
pub fn command(binary: &str) -> Vec<String> {
    command_with(&current().words, binary)
}

/// Whether agents start through a wrapper, which then provides their sandbox.
pub fn active() -> bool {
    !current().words.is_empty()
}

/// Whether Codex may start its own sandbox: not inside a wrapper's, unless asked for.
pub fn codex_sandbox() -> bool {
    !active() || get().codex_sandbox
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
    fn refuses_a_wrapper_without_the_feature() {
        assert!(load("sandbox", false).err().unwrap().contains("agent-wrapper"));
        assert!(load(" ", false).unwrap().words.is_empty());
    }

    #[cfg(feature = "agent-wrapper")]
    #[test]
    fn reads_the_wrapper() {
        let c = load(" sandbox --net ", true).unwrap();
        assert_eq!(c.words, ["sandbox", "--net"]);
        assert_eq!(c.wrapper, AgentWrapper { command: "sandbox --net".into(), codex_sandbox: true });
        assert!(load("sandbox 'oops", false).is_err());
    }
}
