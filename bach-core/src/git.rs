//! Branch info and per-session workspaces (plain checkout or an isolated git worktree).
//! Shells out to `git`, which must be on the backend host's PATH.
use crate::fs::expand_home;
use serde::Serialize;
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Serialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct GitInfo {
    pub is_repo: bool,
    pub root: Option<String>,
    /// None when HEAD is detached.
    pub current: Option<String>,
    /// Local branches, most recently committed first.
    pub branches: Vec<String>,
    /// Tracked files have uncommitted changes (untracked files are ignored).
    pub dirty: bool,
}

#[derive(Serialize, Debug)]
pub struct Workspace {
    /// The directory the agent should run in.
    pub workdir: String,
    pub branch: Option<String>,
    pub worktree: bool,
}

/// Where new worktrees are created: `<worktrees_dir>/<repo>-<hash>/<branch>`. Kept outside
/// the repository so its status stays clean.
#[derive(Clone)]
pub struct Git {
    worktrees_dir: PathBuf,
}

fn run(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| format!("failed to run git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn folder(path: &str) -> Result<PathBuf, String> {
    let dir = expand_home(path.trim());
    if dir.is_dir() {
        Ok(dir)
    } else {
        Err(format!(
            "{} is not a folder on the machine running the agents.",
            dir.display()
        ))
    }
}

fn info_sync(path: &str) -> Result<GitInfo, String> {
    let dir = folder(path)?;
    let Ok(root) = run(&dir, &["rev-parse", "--show-toplevel"]) else {
        return Ok(GitInfo::default());
    };
    let current = Some(run(&dir, &["branch", "--show-current"])?).filter(|b| !b.is_empty());
    let branches = run(
        &dir,
        &[
            "for-each-ref",
            "--sort=-committerdate",
            "--format=%(refname:short)",
            "refs/heads",
        ],
    )?
    .lines()
    .map(str::to_string)
    .collect();
    let dirty = !run(&dir, &["status", "--porcelain", "--untracked-files=no"])?.is_empty();
    Ok(GitInfo {
        is_repo: true,
        root: Some(root),
        current,
        branches,
        dirty,
    })
}

impl Git {
    pub fn new(worktrees_dir: PathBuf) -> Self {
        Self { worktrees_dir }
    }

    pub async fn info(&self, path: String) -> Result<GitInfo, String> {
        tokio::task::spawn_blocking(move || info_sync(&path))
            .await
            .map_err(|e| e.to_string())?
    }

    /// Readies the directory a new session will run in.
    /// - `worktree == false`: runs in `cwd`, first switching it to `branch` if that differs.
    /// - `worktree == true`: creates `new_branch` from `branch` (default: current) in a fresh worktree.
    pub async fn prepare(
        &self,
        cwd: String,
        branch: Option<String>,
        worktree: bool,
        new_branch: Option<String>,
    ) -> Result<Workspace, String> {
        let dirs = self.worktrees_dir.clone();
        tokio::task::spawn_blocking(move || prepare_sync(&dirs, &cwd, branch, worktree, new_branch))
            .await
            .map_err(|e| e.to_string())?
    }
}

fn prepare_sync(
    worktrees_dir: &Path,
    cwd: &str,
    branch: Option<String>,
    worktree: bool,
    new_branch: Option<String>,
) -> Result<Workspace, String> {
    let dir = folder(cwd)?;
    let info = info_sync(cwd)?;
    let workdir_here = |branch| Workspace {
        workdir: dir.to_string_lossy().into_owned(),
        branch,
        worktree: false,
    };

    if !info.is_repo {
        return if worktree || branch.is_some() {
            Err(format!("{} is not a git repository.", dir.display()))
        } else {
            Ok(workdir_here(None))
        };
    }
    // Only known branches are accepted, which also keeps option-like names away from git.
    if let Some(b) = &branch {
        if !info.branches.contains(b) {
            return Err(format!("Unknown branch `{b}`."));
        }
    }

    if !worktree {
        return match branch {
            Some(b) if info.current.as_ref() != Some(&b) => {
                run(&dir, &["switch", "--end-of-options", &b])?;
                Ok(workdir_here(Some(b)))
            }
            other => Ok(workdir_here(other.or(info.current))),
        };
    }

    let name = new_branch.ok_or("A branch name is required to create a worktree.")?;
    if name.starts_with('-') || run(&dir, &["check-ref-format", "--branch", &name]).is_err() {
        return Err(format!("`{name}` isn't a valid branch name."));
    }
    if info.branches.contains(&name) {
        return Err(format!("Branch `{name}` already exists."));
    }
    let base = branch.or(info.current).unwrap_or_else(|| "HEAD".into());

    let root = PathBuf::from(info.root.expect("repo has a root"));
    let mut h = DefaultHasher::new();
    root.hash(&mut h);
    let repo = root
        .file_name()
        .map_or("repo".into(), |n| n.to_string_lossy().into_owned());
    let path = worktrees_dir
        .join(format!("{repo}-{:08x}", h.finish() as u32))
        .join(name.replace('/', "-"));
    std::fs::create_dir_all(path.parent().expect("has parent")).map_err(|e| e.to_string())?;
    run(
        &root,
        &[
            "worktree",
            "add",
            "-b",
            &name,
            &path.to_string_lossy(),
            &base,
        ],
    )?;

    Ok(Workspace {
        workdir: path.to_string_lossy().into_owned(),
        branch: Some(name),
        worktree: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(dir: &Path, args: &[&str]) {
        let ok = Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .arg("-C")
            .arg(dir)
            .args(args)
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?} failed");
    }

    fn repo(name: &str) -> (PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!("bach-git-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let repo = base.join("proj");
        std::fs::create_dir_all(&repo).unwrap();
        sh(&repo, &["init", "-q", "-b", "main"]);
        std::fs::write(repo.join("a.txt"), "a").unwrap();
        sh(&repo, &["add", "."]);
        sh(&repo, &["commit", "-q", "-m", "one"]);
        sh(&repo, &["branch", "dev"]);
        (base, repo)
    }

    #[test]
    fn reports_branches_and_non_repos() {
        let (base, repo) = repo("info");
        let info = info_sync(repo.to_str().unwrap()).unwrap();
        assert!(info.is_repo && !info.dirty);
        assert_eq!(info.current.as_deref(), Some("main"));
        assert_eq!(info.branches.len(), 2);

        std::fs::write(repo.join("a.txt"), "changed").unwrap();
        assert!(info_sync(repo.to_str().unwrap()).unwrap().dirty);

        let plain = base.join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        assert!(!info_sync(plain.to_str().unwrap()).unwrap().is_repo);
        assert!(info_sync("/definitely/not/here").is_err());
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn switches_branch_without_worktree() {
        let (base, repo) = repo("switch");
        let ws = prepare_sync(
            &base.join("wt"),
            repo.to_str().unwrap(),
            Some("dev".into()),
            false,
            None,
        )
        .unwrap();
        assert_eq!(ws.workdir, repo.to_str().unwrap());
        assert_eq!(
            info_sync(repo.to_str().unwrap())
                .unwrap()
                .current
                .as_deref(),
            Some("dev")
        );
        assert!(prepare_sync(
            &base.join("wt"),
            repo.to_str().unwrap(),
            Some("nope".into()),
            false,
            None
        )
        .is_err());
        assert!(prepare_sync(
            &base.join("wt"),
            repo.to_str().unwrap(),
            Some("--orphan".into()),
            false,
            None
        )
        .is_err());
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn creates_an_isolated_worktree_on_a_new_branch() {
        let (base, repo) = repo("worktree");
        let wts = base.join("wt");
        let ws = prepare_sync(
            &wts,
            repo.to_str().unwrap(),
            Some("dev".into()),
            true,
            Some("bach/fix-x".into()),
        )
        .unwrap();
        assert!(ws.worktree && ws.branch.as_deref() == Some("bach/fix-x"));
        assert!(Path::new(&ws.workdir).starts_with(&wts));
        assert!(Path::new(&ws.workdir).join("a.txt").exists());
        // The worktree is on the new branch; the original checkout is untouched.
        assert_eq!(
            info_sync(&ws.workdir).unwrap().current.as_deref(),
            Some("bach/fix-x")
        );
        assert_eq!(
            info_sync(repo.to_str().unwrap())
                .unwrap()
                .current
                .as_deref(),
            Some("main")
        );

        let again = |name: Option<&str>| {
            prepare_sync(
                &wts,
                repo.to_str().unwrap(),
                None,
                true,
                name.map(String::from),
            )
        };
        assert!(again(Some("bach/fix-x")).is_err(), "existing branch name");
        assert!(again(Some("bad name")).is_err(), "invalid ref");
        assert!(again(Some("-x")).is_err(), "option-like");
        assert!(again(None).is_err(), "name required");
        let _ = std::fs::remove_dir_all(base);
    }
}
