//! Branch info and per-session workspaces (plain checkout or an isolated git worktree).
//! Shells out to `git`, which must be on the backend host's PATH.
use crate::fs::expand_home;
pub use bach_protocol::{
    DiffHunk, DiffLine, FileDiff, FileStatus, GitDiff, GitInfo, LineKind, Workspace, WorktreeEntry,
};
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    process::Command,
};

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

    pub async fn diff(&self, path: String, base_branch: Option<String>) -> Result<GitDiff, String> {
        tokio::task::spawn_blocking(move || diff_sync(&path, base_branch.as_deref()))
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
    let merged = branch.as_deref().is_some_and(|b| is_merged(&root, b));
    let repo = root
        .file_name()
        .map_or("repo".into(), |n| n.to_string_lossy().into_owned());
    let entry = WorktreeEntry {
        path: path.to_string_lossy().into_owned(),
        repo,
        branch,
        dirty,
        unmerged,
        merged,
    };
    Some((entry, root))
}

/// Whether `branch`'s changes are already in the repository's base branch. That holds for a true
/// merge, and also for a squash or rebase merge, which leave the commits unreachable: merging the
/// branch into the base would then change nothing. A branch level with the base has nothing to
/// merge yet (a fresh worktree), so it doesn't count.
fn is_merged(root: &Path, branch: &str) -> bool {
    let branch_ref = format!("refs/heads/{branch}");
    let Ok(tip) = run(root, &["rev-parse", &branch_ref]) else {
        return false;
    };
    // The remote's default first: local main is often stale once a PR merges on the server.
    let mut bases = vec![];
    if let Ok(head) = run(
        root,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    ) {
        bases.push(head);
    }
    for b in ["main", "master"] {
        bases.push(format!("refs/heads/{b}"));
    }
    bases.into_iter().any(|base| {
        if base == branch_ref || base.strip_prefix("refs/heads/") == Some(branch) {
            return false;
        }
        let (Ok(base_tip), Ok(base_tree)) = (
            run(
                root,
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("{base}^{{commit}}"),
                ],
            ),
            run(
                root,
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("{base}^{{tree}}"),
                ],
            ),
        ) else {
            return false;
        };
        if base_tip == tip {
            return false;
        }
        // A conflict exits non-zero, which is an Err here: not merged.
        run(root, &["merge-tree", "--write-tree", &base, &branch_ref])
            .is_ok_and(|tree| tree == base_tree)
    })
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

    if !discard && (entry.dirty || (entry.unmerged > 0 && !entry.merged)) {
        let mut why = vec![];
        if entry.dirty {
            why.push("uncommitted changes".to_string());
        }
        if entry.unmerged > 0 && !entry.merged {
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
        // A repo with no commits yet has a current branch that isn't a real ref (and so isn't
        // listed), but running on it is fine.
        if !info.branches.contains(b) && info.current.as_ref() != Some(b) {
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

/// A file with more changed lines than this is listed without its lines.
const MAX_FILE_LINES: usize = 5_000;
/// Once the diff holds this many lines, the remaining files are listed without theirs.
const MAX_TOTAL_LINES: usize = 40_000;
/// Untracked files beyond this many aren't shown (a missing `.gitignore` can mean thousands).
const MAX_UNTRACKED: usize = 500;

/// Like [`run`], but for commands that exit with 1 to say "there were differences".
fn run_diff(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        // Whatever the user's config says: plain output, `a/` `b/` prefixes, unquoted paths.
        .args(["-c", "core.quotePath=false", "-c", "diff.noprefix=false"])
        .args(args)
        .output()
        .map_err(|e| format!("failed to run git: {e}"))?;
    match out.status.code() {
        Some(0 | 1) => Ok(String::from_utf8_lossy(&out.stdout).into_owned()),
        _ => Err(String::from_utf8_lossy(&out.stderr).trim().to_string()),
    }
}

const DIFF_OPTS: [&str; 6] = [
    "--no-color",
    "--no-ext-diff",
    "--no-textconv",
    "--find-renames",
    "--src-prefix=a/",
    "--dst-prefix=b/",
];

fn diff_sync(path: &str, base_branch: Option<&str>) -> Result<GitDiff, String> {
    let dir = folder(path)?;
    let info = info_sync(path)?;
    if !info.is_repo {
        return Err(format!("{} is not a git repository.", dir.display()));
    }
    let head = run(&dir, &["rev-parse", "--verify", "--quiet", "HEAD"]).ok();
    let base = match base_branch {
        // Only known branches, which also keeps option-like names away from git.
        Some(b) if !info.branches.iter().any(|x| x == b) => {
            return Err(format!("Unknown branch `{b}`."))
        }
        Some(b) if head.is_some() => Some(run(&dir, &["merge-base", b, "HEAD"])?),
        _ => head,
    };
    // Before the first commit, everything is compared with the empty tree.
    let against = match &base {
        Some(c) => c.clone(),
        None => run(&dir, &["hash-object", "-t", "tree", "/dev/null"])?,
    };

    let mut args = vec!["diff"];
    args.extend(DIFF_OPTS);
    args.extend([against.as_str(), "--"]);
    let mut files = parse_diff(&run_diff(&dir, &args)?);

    let untracked = run(&dir, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    for name in untracked
        .split('\0')
        .filter(|n| !n.is_empty())
        .take(MAX_UNTRACKED)
    {
        let mut args = vec!["diff", "--no-index"];
        args.extend(DIFF_OPTS);
        args.extend(["--", "/dev/null", name]);
        for mut f in parse_diff(&run_diff(&dir, &args)?) {
            f.path = name.to_string();
            f.untracked = true;
            files.push(f);
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));

    let mut total = 0;
    let mut truncated = false;
    for f in &mut files {
        let lines: usize = f.hunks.iter().map(|h| h.lines.len()).sum();
        if lines > MAX_FILE_LINES || total + lines > MAX_TOTAL_LINES {
            f.hunks.clear();
            f.omitted = true;
            truncated = true;
        } else {
            total += lines;
        }
    }

    let base = match base {
        Some(c) => Some(run(&dir, &["rev-parse", "--short", &c])?),
        None => None,
    };
    Ok(GitDiff {
        base,
        files,
        truncated,
    })
}

/// A path from a `--- a/x` / `+++ b/x` line; None for `/dev/null`.
fn diff_path(s: &str) -> Option<String> {
    // Git appends a tab to names containing spaces.
    let s = s.strip_suffix('\t').unwrap_or(s);
    if s == "/dev/null" {
        return None;
    }
    let s = s
        .strip_prefix("a/")
        .or_else(|| s.strip_prefix("b/"))
        .unwrap_or(s);
    Some(s.to_string())
}

/// `@@ -12,3 +14,5 @@` → (12, 14)
fn hunk_starts(header: &str) -> (u32, u32) {
    let mut parts = header.split_whitespace().skip(1);
    let mut start = |sign: char| {
        parts
            .next()
            .and_then(|p| p.strip_prefix(sign))
            .and_then(|p| p.split(',').next())
            .and_then(|n| n.parse().ok())
            .unwrap_or(1)
    };
    let old = start('-');
    (old, start('+'))
}

/// Parses `git diff` output (unified, with `a/` and `b/` prefixes and unquoted paths).
fn parse_diff(text: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = vec![];
    let (mut old_no, mut new_no) = (0, 0);
    let mut in_header = false;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            // `a/x b/x`: both halves are the same length unless renamed (then `rename to`
            // or `+++` says which is which).
            let half = rest.len().saturating_sub(1) / 2;
            let guess = rest.get(half + 1..).and_then(diff_path).unwrap_or_default();
            files.push(FileDiff {
                path: guess,
                old_path: None,
                status: FileStatus::Modified,
                untracked: false,
                binary: false,
                additions: 0,
                deletions: 0,
                hunks: vec![],
                omitted: false,
            });
            in_header = true;
            continue;
        }
        let Some(f) = files.last_mut() else { continue };
        if in_header {
            if line.starts_with("new file mode") {
                f.status = FileStatus::Added;
            } else if line.starts_with("deleted file mode") {
                f.status = FileStatus::Deleted;
            } else if let Some(p) = line.strip_prefix("rename from ") {
                f.status = FileStatus::Renamed;
                f.old_path = Some(p.to_string());
            } else if let Some(p) = line.strip_prefix("rename to ") {
                f.path = p.to_string();
            } else if line.starts_with("Binary files ") {
                f.binary = true;
            } else if let Some(p) = line.strip_prefix("--- ") {
                if let (Some(p), FileStatus::Deleted) = (diff_path(p), f.status) {
                    f.path = p;
                }
            } else if let Some(p) = line.strip_prefix("+++ ") {
                if let Some(p) = diff_path(p) {
                    f.path = p;
                }
            }
        }
        if line.starts_with("@@") {
            in_header = false;
            (old_no, new_no) = hunk_starts(line);
            f.hunks.push(DiffHunk {
                header: line.to_string(),
                lines: vec![],
            });
            continue;
        }
        if in_header {
            continue;
        }
        let Some(h) = f.hunks.last_mut() else {
            continue;
        };
        let (kind, text) = match line.split_at_checked(1) {
            Some(("+", t)) => (LineKind::Add, t),
            Some(("-", t)) => (LineKind::Delete, t),
            Some((" ", t)) => (LineKind::Context, t),
            Some(("\\", _)) => {
                if let Some(l) = h.lines.last_mut() {
                    l.no_newline = true;
                }
                continue;
            }
            // An empty context line (some tools strip the trailing space).
            _ => (LineKind::Context, ""),
        };
        let (old, new) = match kind {
            LineKind::Add => (None, Some(new_no)),
            LineKind::Delete => (Some(old_no), None),
            LineKind::Context => (Some(old_no), Some(new_no)),
        };
        if old.is_some() {
            old_no += 1;
        }
        if new.is_some() {
            new_no += 1;
        }
        match kind {
            LineKind::Add => f.additions += 1,
            LineKind::Delete => f.deletions += 1,
            LineKind::Context => {}
        }
        h.lines.push(DiffLine {
            kind,
            text: text.to_string(),
            old,
            new,
            no_newline: false,
        });
    }
    files
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

    #[test]
    fn detects_merged_branches() {
        let (base, repo) = repo("merged");
        let wts = base.join("wt");
        let mk = |name: &str| {
            let w = prepare_sync(&wts, repo.to_str().unwrap(), None, true, Some(name.into()))
                .unwrap()
                .workdir;
            let w = PathBuf::from(w);
            for i in 0..2 {
                std::fs::write(w.join(format!("{}{i}.txt", name.replace('/', "-"))), "x").unwrap();
                sh(&w, &["add", "."]);
                sh(&w, &["commit", "-q", "-m", &format!("{name} {i}")]);
            }
        };
        for n in ["bach/squash", "bach/rebase", "bach/real", "bach/open"] {
            mk(n);
        }
        sh(&repo, &["merge", "-q", "--squash", "bach/squash"]);
        sh(&repo, &["commit", "-q", "-m", "squashed"]);
        sh(&repo, &["cherry-pick", "bach/rebase~1", "bach/rebase"]);
        sh(
            &repo,
            &["merge", "-q", "--no-ff", "-m", "merge", "bach/real"],
        );

        prepare_sync(
            &wts,
            repo.to_str().unwrap(),
            None,
            true,
            Some("bach/fresh".into()),
        )
        .unwrap();

        let list = list_worktrees_sync(&wts);
        let merged = |b: &str| {
            list.iter()
                .find(|e| e.branch.as_deref() == Some(b))
                .unwrap()
                .merged
        };
        // Their commits are on no other branch, but the changes are in main: safe to remove.
        let squash = list.iter().find(|e| e.branch.as_deref() == Some("bach/squash"));
        assert!(squash.unwrap().unmerged > 0);
        let path = squash.unwrap().path.clone();
        remove_worktree_sync(&wts, &path, false, true).unwrap();
        let list = list_worktrees_sync(&wts);
        let merged = |b: &str| {
            list.iter()
                .find(|e| e.branch.as_deref() == Some(b))
                .map(|e| e.merged)
                .unwrap_or(true)
        };
        assert!(merged("bach/squash"), "squash merge");
        assert!(merged("bach/rebase"), "rebase merge");
        assert!(merged("bach/real"), "true merge");
        assert!(!merged("bach/open"), "still has unique changes");
        assert!(!merged("bach/fresh"), "nothing committed yet");
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn diffs_uncommitted_and_branch_changes() {
        let (base, repo) = repo("diff");
        let path = repo.to_str().unwrap();
        assert!(diff_sync(path, None).unwrap().files.is_empty());

        std::fs::write(repo.join("a.txt"), "a\nb\n").unwrap();
        std::fs::write(repo.join("gone.txt"), "x\n").unwrap();
        sh(&repo, &["add", "."]);
        sh(&repo, &["commit", "-q", "-m", "two"]);
        sh(&repo, &["switch", "-q", "-c", "feature"]);
        std::fs::write(repo.join("c.txt"), "committed\n").unwrap();
        sh(&repo, &["add", "."]);
        sh(&repo, &["commit", "-q", "-m", "three"]);
        std::fs::write(repo.join("a.txt"), "a\nB\nc").unwrap();
        std::fs::remove_file(repo.join("gone.txt")).unwrap();
        std::fs::write(repo.join("new file.txt"), "n\n").unwrap();

        let d = diff_sync(path, None).unwrap();
        let by = |p: &str| d.files.iter().find(|f| f.path == p).unwrap();
        assert_eq!(d.files.len(), 3, "{:?}", d.files);
        let a = by("a.txt");
        assert_eq!(
            (a.status, a.additions, a.deletions),
            (FileStatus::Modified, 2, 1)
        );
        let lines = &a.hunks[0].lines;
        assert_eq!(lines[0].kind, LineKind::Context);
        assert_eq!(
            (lines[1].kind, lines[1].old, lines[1].text.as_str()),
            (LineKind::Delete, Some(2), "b")
        );
        assert_eq!((lines[2].kind, lines[2].new), (LineKind::Add, Some(2)));
        assert!(lines[3].no_newline && lines[3].new == Some(3));
        assert_eq!(by("gone.txt").status, FileStatus::Deleted);
        let n = by("new file.txt");
        assert!(n.untracked && n.status == FileStatus::Added && n.additions == 1);

        // Against the branch it came from: the committed file shows up too.
        let d = diff_sync(path, Some("main")).unwrap();
        assert_eq!(d.files.len(), 4);
        assert!(d
            .files
            .iter()
            .any(|f| f.path == "c.txt" && f.status == FileStatus::Added));
        assert!(diff_sync(path, Some("--all")).is_err());
        assert!(diff_sync(path, Some("nope")).is_err());

        sh(&repo, &["add", "-A"]);
        sh(&repo, &["commit", "-q", "-m", "four"]);
        sh(&repo, &["mv", "c.txt", "d.txt"]);
        let d = diff_sync(path, None).unwrap();
        assert_eq!(d.files.len(), 1);
        assert_eq!(d.files[0].status, FileStatus::Renamed);
        assert_eq!(
            (d.files[0].path.as_str(), d.files[0].old_path.as_deref()),
            ("d.txt", Some("c.txt"))
        );
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn works_in_a_repo_with_no_commits() {
        let base = std::env::temp_dir().join(format!("bach-git-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        sh(&base, &["init", "-q", "-b", "main"]);
        let path = base.to_str().unwrap();

        let info = info_sync(path).unwrap();
        assert!(info.is_repo && info.branches.is_empty());
        assert_eq!(info.current.as_deref(), Some("main"));

        // The default selection (the current, unborn branch) is accepted as is.
        let ws = prepare_sync(&base.join("wt"), path, Some("main".into()), false, None).unwrap();
        assert_eq!(ws.workdir, path);
        // An unrelated unknown branch is still refused.
        assert!(prepare_sync(&base.join("wt"), path, Some("nope".into()), false, None).is_err());

        // Its diff is everything there, against nothing.
        std::fs::write(base.join("a.txt"), "a\n").unwrap();
        let d = diff_sync(path, None).unwrap();
        assert!(d.base.is_none() && d.files.len() == 1 && d.files[0].untracked);
        let _ = std::fs::remove_dir_all(base);
    }
}
