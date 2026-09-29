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

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeEntry {
    pub path: String,
    /// Folder name of the repository it belongs to.
    pub repo: String,
    pub branch: Option<String>,
    /// Uncommitted changes, including untracked files: removing the worktree would lose them.
    pub dirty: bool,
    /// Commits on the branch that exist on no other local or remote branch.
    pub unmerged: u32,
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

    pub async fn list_worktrees(&self) -> Vec<WorktreeEntry> {
        let dir = self.worktrees_dir.clone();
        tokio::task::spawn_blocking(move || list_worktrees_sync(&dir))
            .await
            .unwrap_or_default()
    }

    /// Removes a worktree Bach created. Refuses if it holds work that exists nowhere else,
    /// unless `discard` is set.
    pub async fn remove_worktree(
        &self,
        path: String,
        discard: bool,
        delete_branch: bool,
    ) -> Result<(), String> {
        let dir = self.worktrees_dir.clone();
        tokio::task::spawn_blocking(move || {
            remove_worktree_sync(&dir, &path, discard, delete_branch)
        })
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

/// Describes a directory under the worktrees folder, or None if it isn't a live worktree.
fn worktree_entry(path: &Path) -> Option<(WorktreeEntry, PathBuf)> {
    let common = run(
        path,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .ok()?;
    let root = PathBuf::from(common).parent()?.to_path_buf();
    let branch = run(path, &["branch", "--show-current"])
        .ok()
        .filter(|b| !b.is_empty());
    let dirty = !run(path, &["status", "--porcelain"]).ok()?.is_empty();
    let unmerged = branch
        .as_ref()
        .and_then(|b| {
            run(
                path,
                &[
                    "rev-list",
                    "--count",
                    &format!("refs/heads/{b}"),
                    "--not",
                    &format!("--exclude={b}"),
                    "--branches",
                    "--remotes",
                ],
            )
            .ok()
        })
        .and_then(|n| n.parse().ok())
        .unwrap_or(0);
    let repo = root
        .file_name()
        .map_or("repo".into(), |n| n.to_string_lossy().into_owned());
    let entry = WorktreeEntry {
        path: path.to_string_lossy().into_owned(),
        repo,
        branch,
        dirty,
        unmerged,
    };
    Some((entry, root))
}

/// Worktrees live at `<worktrees_dir>/<repo>-<hash>/<name>`.
fn list_worktrees_sync(worktrees_dir: &Path) -> Vec<WorktreeEntry> {
    let Ok(repos) = std::fs::read_dir(worktrees_dir) else {
        return vec![];
    };
    let mut out: Vec<_> = repos
        .filter_map(Result::ok)
        .filter_map(|r| std::fs::read_dir(r.path()).ok())
        .flatten()
        .filter_map(Result::ok)
        .filter(|w| w.path().is_dir())
        .filter_map(|w| worktree_entry(&w.path()).map(|(e, _)| e))
        .collect();
    out.sort_by(|a, b| (&a.repo, &a.branch).cmp(&(&b.repo, &b.branch)));
    out
}

fn remove_worktree_sync(
    worktrees_dir: &Path,
    path: &str,
    discard: bool,
    delete_branch: bool,
) -> Result<(), String> {
    // Only ever touch what Bach itself created: exactly <worktrees_dir>/<repo>/<name>.
    let base = worktrees_dir.canonicalize().map_err(|e| e.to_string())?;
    let target = Path::new(path)
        .canonicalize()
        .map_err(|e| format!("{path}: {e}"))?;
    if target.parent().and_then(Path::parent) != Some(base.as_path()) {
        return Err("That folder isn't a worktree created by Bach.".into());
    }
    let (entry, root) = worktree_entry(&target).ok_or("That folder isn't a valid git worktree.")?;

    if !discard && (entry.dirty || entry.unmerged > 0) {
        let mut why = vec![];
        if entry.dirty {
            why.push("uncommitted changes".to_string());
        }
        if entry.unmerged > 0 {
            why.push(format!(
                "{} commit(s) that exist on no other branch",
                entry.unmerged
            ));
        }
        return Err(format!("Not removed: it has {}.", why.join(" and ")));
    }

    let target_str = target.to_string_lossy();
    let mut args = vec!["worktree", "remove"];
    if discard {
        args.push("--force");
    }
    args.push(&target_str);
    run(&root, &args)?;

    if delete_branch {
        if let Some(b) = &entry.branch {
            run(&root, &["branch", "-D", b])?;
        }
    }
    // Drop the per-repo folder once it's empty (fails harmlessly otherwise).
    let _ = target.parent().map(std::fs::remove_dir);
    Ok(())
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

    #[test]
    fn lists_and_removes_worktrees_safely() {
        let (base, repo) = repo("cleanup");
        let wts = base.join("wt");
        let mk = |name: &str| {
            prepare_sync(&wts, repo.to_str().unwrap(), None, true, Some(name.into()))
                .unwrap()
                .workdir
        };
        let clean = mk("bach/clean");
        let dirty = mk("bach/dirty");
        let ahead = mk("bach/ahead");
        std::fs::write(Path::new(&dirty).join("new.txt"), "x").unwrap(); // untracked counts
        std::fs::write(Path::new(&ahead).join("b.txt"), "b").unwrap();
        sh(Path::new(&ahead), &["add", "."]);
        sh(Path::new(&ahead), &["commit", "-q", "-m", "work"]);

        let list = list_worktrees_sync(&wts);
        let by = |b: &str| {
            list.iter()
                .find(|e| e.branch.as_deref() == Some(b))
                .unwrap()
        };
        assert_eq!(list.len(), 3);
        assert!(!by("bach/clean").dirty && by("bach/clean").unmerged == 0);
        assert!(by("bach/dirty").dirty);
        assert_eq!(by("bach/ahead").unmerged, 1);
        assert_eq!(by("bach/clean").repo, "proj");

        // Unsafe removals are refused unless discard is set; nothing is lost on refusal.
        assert!(remove_worktree_sync(&wts, &dirty, false, true)
            .unwrap_err()
            .contains("uncommitted"));
        assert!(remove_worktree_sync(&wts, &ahead, false, true)
            .unwrap_err()
            .contains("no other branch"));
        assert!(Path::new(&dirty).exists() && Path::new(&ahead).exists());

        // Paths outside the worktrees folder are never touched, including the main checkout.
        assert!(remove_worktree_sync(&wts, repo.to_str().unwrap(), true, true).is_err());
        assert!(remove_worktree_sync(&wts, "/tmp", true, true).is_err());
        assert!(repo.join("a.txt").exists());

        // A clean one goes, and its branch with it.
        remove_worktree_sync(&wts, &clean, false, true).unwrap();
        assert!(!Path::new(&clean).exists());
        assert!(!info_sync(repo.to_str().unwrap())
            .unwrap()
            .branches
            .contains(&"bach/clean".to_string()));

        // Discarding removes the rest; keeping the branch works too.
        remove_worktree_sync(&wts, &dirty, true, false).unwrap();
        assert!(info_sync(repo.to_str().unwrap())
            .unwrap()
            .branches
            .contains(&"bach/dirty".to_string()));
        remove_worktree_sync(&wts, &ahead, true, true).unwrap();
        assert!(list_worktrees_sync(&wts).is_empty());
        let _ = std::fs::remove_dir_all(base);
    }
}
