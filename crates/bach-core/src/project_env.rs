//! The project's own environment, from its direnv `.envrc`. Agents start with the server's PATH,
//! so without it a project whose tools come from a dev shell (Nix, say) would start Claude Code
//! with none of them, and neither would the MCP servers Claude Code starts in turn. Only an
//! `.envrc` the user has `direnv allow`ed is loaded, as in their terminal.
use serde::Deserialize;
use std::{collections::HashMap, path::Path, process::Stdio, time::Duration};
use tokio::process::Command;

/// Changes to the environment: a variable to set, or (`None`) to unset.
#[derive(Debug, Default, Deserialize)]
#[serde(transparent)]
pub struct Changes(HashMap<String, Option<String>>);

impl Changes {
    /// Applies them to `cmd`, before anything the caller sets itself.
    pub fn apply(&self, cmd: &mut Command) {
        for (k, v) in &self.0 {
            match v {
                Some(v) => cmd.env(k, v),
                None => cmd.env_remove(k),
            };
        }
    }
}

/// A dev shell built for the first time can take minutes; an agent shouldn't wait that long.
const TIMEOUT: Duration = Duration::from_secs(30);

/// What direnv would change on entering `dir`: nothing without an `.envrc`, without direnv, or
/// if it isn't allowed, fails or takes too long.
pub async fn load(dir: &Path) -> Changes {
    export(dir, &[]).await
}

async fn export(dir: &Path, env: &[(&str, &Path)]) -> Changes {
    if !dir.join(".envrc").is_file() || which::which("direnv").is_err() {
        return Changes::default();
    }
    let out = Command::new("direnv")
        .args(["export", "json"])
        .envs(env.iter().copied())
        .current_dir(dir)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output();
    match tokio::time::timeout(TIMEOUT, out).await {
        Ok(Ok(out)) if out.status.success() => {
            // Nothing to change prints nothing at all.
            serde_json::from_slice(&out.stdout).unwrap_or_default()
        }
        _ => Changes::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn loads_an_allowed_envrc() {
        let dir = std::env::temp_dir().join(format!("bach-envrc-{}", uuid::Uuid::new_v4()));
        let project = dir.join("project");
        std::fs::create_dir_all(&project).unwrap();
        assert!(load(&project).await.0.is_empty(), "no .envrc");
        if which::which("direnv").is_err() {
            return;
        }
        std::fs::write(project.join(".envrc"), "export BACH_ENVRC_TEST=yes\n").unwrap();
        // direnv's record of what's allowed, kept out of the user's own.
        let data = dir.join("data");
        let config = dir.join("config");
        let env = [("XDG_DATA_HOME", data.as_path()), ("XDG_CONFIG_HOME", config.as_path())];
        assert!(export(&project, &env).await.0.get("BACH_ENVRC_TEST").is_none(), "not allowed");

        let allowed = std::process::Command::new("direnv")
            .arg("allow")
            .envs(env)
            .current_dir(&project)
            .status()
            .unwrap();
        assert!(allowed.success());
        let changes = export(&project, &env).await;
        assert_eq!(changes.0.get("BACH_ENVRC_TEST"), Some(&Some("yes".to_string())));
        let _ = std::fs::remove_dir_all(dir);
    }
}
