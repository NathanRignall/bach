//! Branch info and per-session workspaces (plain checkout or an isolated git worktree).
//! Shells out to `git`, which must be on the backend host's PATH.
use crate::fs::expand_home;
pub use bach_protocol::{
    BranchStatus, CommitInfo, DiffHunk, DiffLine, FileDiff, FileStatus, GitDiff, GitInfo, GitLog,
    LineKind, Workspace, WorktreeEntry,
};
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    io::Write,
    process::{Command, Stdio},
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

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(f).await.map_err(|e| e.to_string())?
}

/// Runs a git command that changes the repository, with `input` on its stdin. Never waits for
/// a password or passphrase, and retries once when another git command held the index lock.
/// Fails with what git said (its stderr, or else its stdout).
fn run_write(dir: &Path, args: &[&str], input: Option<&[u8]>) -> Result<String, String> {
    let attempt = || -> Result<(bool, String, String), String> {
        let mut cmd = Command::new("git");
        cmd.arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GCM_INTERACTIVE", "never")
            .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| format!("failed to run git: {e}"))?;
        // Written from another thread so a large patch can't block against a full output pipe.
        let writer = input.map(|data| {
            let mut stdin = child.stdin.take().expect("piped");
            let data = data.to_vec();
            std::thread::spawn(move || {
                let _ = stdin.write_all(&data);
            })
        });
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        if let Some(w) = writer {
            let _ = w.join();
        }
        Ok((
            out.status.success(),
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ))
    };
    let mut result = attempt()?;
    if !result.0 && result.2.contains("index.lock") {
        std::thread::sleep(std::time::Duration::from_millis(300));
        result = attempt()?;
    }
    let (ok, stdout, stderr) = result;
    if ok {
        Ok(stdout)
    } else if stderr.is_empty() || stdout.is_empty() {
        Err(format!("{stdout}{stderr}"))
    } else {
        Err(format!("{stdout}\n{stderr}"))
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

    pub async fn diff(
        &self,
        path: String,
        base_branch: Option<String>,
        commit: Option<String>,
    ) -> Result<GitDiff, String> {
        blocking(move || diff_sync(&path, base_branch.as_deref(), commit.as_deref())).await
    }

    pub async fn log(
        &self,
        path: String,
        base_branch: Option<String>,
        older: bool,
        skip: u32,
        limit: u32,
    ) -> Result<GitLog, String> {
        blocking(move || log_sync(&path, base_branch.as_deref(), older, skip, limit)).await
    }

    pub async fn status(&self, path: String) -> Result<BranchStatus, String> {
        blocking(move || status_sync(&path)).await
    }

    pub async fn stage(&self, path: String, files: Vec<String>, stage: bool) -> Result<(), String> {
        blocking(move || stage_sync(&path, &files, stage)).await
    }

    pub async fn stage_hunk(
        &self,
        path: String,
        file: String,
        hunk: u32,
        header: String,
        stage: bool,
    ) -> Result<(), String> {
        blocking(move || stage_hunk_sync(&path, &file, hunk, &header, stage)).await
    }

    pub async fn commit(&self, path: String, message: String) -> Result<CommitInfo, String> {
        blocking(move || commit_sync(&path, &message)).await
    }

    pub async fn push(&self, path: String) -> Result<BranchStatus, String> {
        blocking(move || push_sync(&path)).await
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

/// Files changed by the `git diff` with `args` (its options and what it compares).
fn diff_files(dir: &Path, args: &[&str], staged: bool) -> Result<Vec<FileDiff>, String> {
    let mut full = vec!["diff"];
    full.extend(DIFF_OPTS);
    full.extend(args);
    let mut files = parse_diff(&run_diff(dir, &full)?);
    for f in &mut files {
        f.staged = staged;
    }
    Ok(files)
}

/// A full or abbreviated commit id, checked to name a commit (and so not to be an option).
fn commit_id(dir: &Path, rev: &str) -> Result<String, String> {
    if rev.len() < 4 || rev.len() > 64 || !rev.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("`{rev}` isn't a commit."));
    }
    run(dir, &["rev-parse", "--verify", "--quiet", &format!("{rev}^{{commit}}")])
        .map_err(|_| format!("Unknown commit `{rev}`."))
}

fn diff_sync(path: &str, base_branch: Option<&str>, commit: Option<&str>) -> Result<GitDiff, String> {
    let dir = folder(path)?;
    let info = info_sync(path)?;
    if !info.is_repo {
        return Err(format!("{} is not a git repository.", dir.display()));
    }
    let head = run(&dir, &["rev-parse", "--verify", "--quiet", "HEAD"]).ok();
    // Before the first commit, everything is compared with the empty tree.
    let empty_tree = || run(&dir, &["hash-object", "-t", "tree", "/dev/null"]);

    let (mut files, base, staging) = if let Some(rev) = commit {
        let sha = commit_id(&dir, rev)?;
        let parent = match run(&dir, &["rev-parse", "--verify", "--quiet", &format!("{sha}^")]) {
            Ok(p) => p,
            Err(_) => empty_tree()?,
        };
        (diff_files(&dir, &[&parent, &sha, "--"], false)?, Some(sha), false)
    } else if let Some(b) = base_branch {
        // Only known branches, which also keeps option-like names away from git.
        if !info.branches.iter().any(|x| x == b) {
            return Err(format!("Unknown branch `{b}`."));
        }
        let base = match &head {
            Some(_) => run(&dir, &["merge-base", b, "HEAD"])?,
            None => empty_tree()?,
        };
        let mut files = diff_files(&dir, &[&base, "--"], false)?;
        files.extend(untracked_files(&dir)?);
        (files, head.is_some().then_some(base), false)
    } else {
        let mut files = diff_files(&dir, &["--cached", "--"], true)?;
        files.extend(diff_files(&dir, &["--"], false)?);
        files.extend(untracked_files(&dir)?);
        (files, head, true)
    };
    files.sort_by(|a, b| (&a.path, a.staged).cmp(&(&b.path, b.staged)));

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
        staging,
        files,
        truncated,
    })
}

/// Untracked files, each shown as added in full.
fn untracked_files(dir: &Path) -> Result<Vec<FileDiff>, String> {
    let untracked = run(dir, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    let mut files = vec![];
    for name in untracked
        .split('\0')
        .filter(|n| !n.is_empty())
        .take(MAX_UNTRACKED)
    {
        let mut args = vec!["diff", "--no-index"];
        args.extend(DIFF_OPTS);
        args.extend(["--", "/dev/null", name]);
        for mut f in parse_diff(&run_diff(dir, &args)?) {
            f.path = name.to_string();
            f.untracked = true;
            files.push(f);
        }
    }
    Ok(files)
}

const DEFAULT_LOG_PAGE: u32 = 50;

fn log_sync(
    path: &str,
    base_branch: Option<&str>,
    older: bool,
    skip: u32,
    limit: u32,
) -> Result<GitLog, String> {
    let dir = folder(path)?;
    let info = info_sync(path)?;
    if !info.is_repo {
        return Err(format!("{} is not a git repository.", dir.display()));
    }
    if run(&dir, &["rev-parse", "--verify", "--quiet", "HEAD"]).is_err() {
        return Ok(GitLog::default());
    }
    if let Some(b) = base_branch {
        if !info.branches.iter().any(|x| x == b) {
            return Err(format!("Unknown branch `{b}`."));
        }
    }
    // The commits that are the branch's own; the rest came with the branch it left.
    let own: Option<std::collections::HashSet<String>> = match base_branch {
        Some(b) => Some(
            run(&dir, &["rev-list", "HEAD", "--not", b, "--"])?
                .lines()
                .map(str::to_string)
                .collect(),
        ),
        None => None,
    };
    let limit = if limit == 0 { DEFAULT_LOG_PAGE } else { limit.min(500) };
    let (skip_arg, max_arg) = (format!("--skip={skip}"), format!("--max-count={}", limit + 1));
    let mut args = vec![
        "log",
        "--no-color",
        "--format=%H%x1f%h%x1f%an%x1f%at%x1f%s",
        &skip_arg,
        &max_arg,
        "HEAD",
    ];
    if let (Some(b), false) = (base_branch, older) {
        args.extend(["--not", b]);
    }
    args.push("--");
    let text = run(&dir, &args)?;
    let mut commits: Vec<CommitInfo> = text
        .lines()
        .filter_map(|l| {
            let mut f = l.split('\x1f');
            let sha = f.next()?.to_string();
            Some(CommitInfo {
                on_branch: own.as_ref().is_none_or(|o| o.contains(&sha)),
                sha,
                short: f.next()?.to_string(),
                author: f.next()?.to_string(),
                time: f.next()?.parse().ok()?,
                subject: f.next().unwrap_or_default().to_string(),
            })
        })
        .collect();
    let has_more = commits.len() > limit as usize;
    commits.truncate(limit as usize);
    Ok(GitLog { commits, has_more })
}

fn status_sync(path: &str) -> Result<BranchStatus, String> {
    let dir = folder(path)?;
    let info = info_sync(path)?;
    if !info.is_repo {
        return Err(format!("{} is not a git repository.", dir.display()));
    }
    let branch = info.current;
    let head = run(&dir, &["rev-parse", "--short", "--verify", "--quiet", "HEAD"]).ok();
    let has_remote = !run(&dir, &["remote"])?.is_empty();
    let counts = |range: &str| -> Option<(u32, u32)> {
        let out = run(&dir, &["rev-list", "--left-right", "--count", range, "--"]).ok()?;
        let (a, b) = out.split_once(char::is_whitespace)?;
        Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
    };
    let tracked = branch.as_ref().and_then(|b| {
        let up = run(
            &dir,
            &["for-each-ref", "--format=%(upstream:short)", &format!("refs/heads/{b}")],
        )
        .ok()
        .filter(|u| !u.is_empty())?;
        // An upstream whose branch is gone from the remote counts as none.
        let (ahead, behind) = counts(&format!("HEAD...{up}"))?;
        Some((up, ahead, behind))
    });
    let (upstream, ahead, behind) = match tracked {
        Some((up, a, b)) => (Some(up), a, b),
        None if head.is_some() => {
            let n = run(&dir, &["rev-list", "--count", "HEAD", "--not", "--remotes", "--"])?;
            (None, n.parse().unwrap_or(0), 0)
        }
        None => (None, 0, 0),
    };
    let staged = run(&dir, &["diff", "--cached", "--name-only", "-z"])?
        .split('\0')
        .filter(|n| !n.is_empty())
        .count() as u32;
    Ok(BranchStatus {
        branch,
        head,
        upstream,
        has_remote,
        ahead,
        behind,
        staged,
    })
}

fn stage_sync(path: &str, files: &[String], stage: bool) -> Result<(), String> {
    let dir = folder(path)?;
    if files.is_empty() {
        return Ok(());
    }
    let has_head = run(&dir, &["rev-parse", "--verify", "--quiet", "HEAD"]).is_ok();
    let mut args = vec!["--literal-pathspecs"];
    args.extend(match (stage, has_head) {
        (true, _) => vec!["add", "--"],
        (false, true) => vec!["restore", "--staged", "--"],
        (false, false) => vec!["rm", "--cached", "-q", "-r", "--"],
    });
    args.extend(files.iter().map(String::as_str));
    run_write(&dir, &args, None).map(drop)
}

/// A file's diff as bytes, split into its header and its hunks (each hunk's `@@` line on).
fn split_hunks(diff: &[u8]) -> (Vec<u8>, Vec<Vec<u8>>) {
    let mut header = vec![];
    let mut hunks: Vec<Vec<u8>> = vec![];
    for line in diff.split_inclusive(|&b| b == b'\n') {
        if line.starts_with(b"@@") {
            hunks.push(vec![]);
        }
        match hunks.last_mut() {
            Some(h) => h.extend_from_slice(line),
            None => header.extend_from_slice(line),
        }
    }
    (header, hunks)
}

fn stage_hunk_sync(
    path: &str,
    file: &str,
    hunk: u32,
    header: &str,
    stage: bool,
) -> Result<(), String> {
    let dir = folder(path)?;
    let tracked = run(&dir, &["ls-files", "--error-unmatch", "--", file]).is_ok();
    if stage && !tracked {
        return Err("A new file can only be staged whole.".into());
    }
    let mut args = vec!["--literal-pathspecs", "diff", "--no-renames"];
    args.extend(DIFF_OPTS);
    if !stage {
        args.push("--cached");
    }
    args.extend(["--", file]);
    let out = Command::new("git")
        .arg("-C")
        .arg(&dir)
        .args(["-c", "core.quotePath=false", "-c", "diff.noprefix=false"])
        .args(&args)
        .output()
        .map_err(|e| format!("failed to run git: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    let (head, hunks) = split_hunks(&out.stdout);
    let changed = "The file changed since it was shown; refresh and try again.";
    let raw = hunks.get(hunk as usize).ok_or(changed)?;
    if raw.split(|&b| b == b'\n').next() != Some(header.as_bytes()) {
        return Err(changed.into());
    }
    let mut patch = head;
    patch.extend_from_slice(raw);
    let mut apply = vec!["apply", "--cached", "--whitespace=nowarn"];
    if !stage {
        apply.push("--reverse");
    }
    apply.push("-");
    run_write(&dir, &apply, Some(&patch)).map(drop)
}

fn commit_sync(path: &str, message: &str) -> Result<CommitInfo, String> {
    let dir = folder(path)?;
    if message.trim().is_empty() {
        return Err("Write a commit message.".into());
    }
    if status_sync(path)?.staged == 0 {
        return Err("Nothing is staged to commit.".into());
    }
    run_write(&dir, &["commit", "--quiet", "-F", "-"], Some(message.as_bytes()))?;
    log_sync(path, None, true, 0, 1)?
        .commits
        .pop()
        .ok_or_else(|| "The commit was made, but can't be read back.".into())
}

/// Turns git's complaint from a failed push into something that says what to do.
fn push_error(stderr: &str) -> String {
    let has = |needles: &[&str]| needles.iter().any(|n| stderr.contains(n));
    let summary = if has(&["[rejected]", "non-fast-forward", "fetch first", "stale info"]) {
        "Rejected: the remote branch has commits this branch lacks. Bach never force-pushes; \
         bring those commits in (merge or rebase) and push again."
    } else if has(&[
        "Permission denied",
        "Authentication failed",
        "could not read Username",
        "could not read Password",
        "terminal prompts disabled",
        "Host key verification failed",
        "Invalid username or password",
        "access denied",
        "The requested URL returned error: 403",
    ]) {
        "Not authorised: the machine running Bach couldn't log in to the remote. Check its SSH key or stored credentials."
    } else if has(&["pre-receive hook declined", "protected branch", "remote rejected"]) {
        "The remote refused the push."
    } else if has(&["does not appear to be a git repository", "Repository not found", "not found"]) {
        "The remote repository wasn't found. Check the remote's URL."
    } else if has(&["Could not resolve host", "Connection refused", "Connection timed out", "unable to access", "Could not read from remote repository"]) {
        "Couldn't reach the remote."
    } else {
        "The push failed."
    };
    format!("{summary}\n\n{stderr}")
}

fn push_sync(path: &str) -> Result<BranchStatus, String> {
    let dir = folder(path)?;
    let status = status_sync(path)?;
    let branch = status
        .branch
        .ok_or("HEAD is detached: check out a branch to push it.")?;
    if status.head.is_none() {
        return Err("Nothing to push: the branch has no commits yet.".into());
    }
    if !status.has_remote {
        return Err("This repository has no remote to push to. Add one (git remote add origin <url>) and try again.".into());
    }
    let config = |key: String| run(&dir, &["config", "--get", &key]).ok().filter(|v| !v.is_empty());
    let own_ref = format!("refs/heads/{branch}");
    let mut args: Vec<String> = vec!["push".into()];
    match (
        status.upstream.is_some(),
        config(format!("branch.{branch}.remote")),
        config(format!("branch.{branch}.merge")),
    ) {
        (true, Some(remote), Some(merge)) if remote != "." => {
            args.extend([remote, format!("{own_ref}:{merge}")]);
        }
        _ => {
            let remotes: Vec<String> = run(&dir, &["remote"])?.lines().map(str::to_string).collect();
            let remote = config(format!("branch.{branch}.pushRemote"))
                .or_else(|| config("remote.pushDefault".into()))
                .filter(|r| remotes.contains(r))
                .or_else(|| remotes.iter().find(|r| *r == "origin").cloned())
                .or_else(|| (remotes.len() == 1).then(|| remotes[0].clone()))
                .ok_or_else(|| {
                    format!("Which remote? There are several ({}) and none is `origin`.", remotes.join(", "))
                })?;
            args.extend(["--set-upstream".into(), remote, format!("{own_ref}:{own_ref}")]);
        }
    }
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(&dir).args(&args);
    // A prompt for a passphrase or host key can't be answered here.
    if std::env::var_os("GIT_SSH_COMMAND").is_none() && config("core.sshCommand".into()).is_none() {
        cmd.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
    }
    let out = cmd
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("failed to run git: {e}"))?;
    if !out.status.success() {
        return Err(push_error(String::from_utf8_lossy(&out.stderr).trim()));
    }
    status_sync(path)
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
                staged: false,
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
        assert!(diff_sync(path, None, None).unwrap().files.is_empty());

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

        let d = diff_sync(path, None, None).unwrap();
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
        let d = diff_sync(path, Some("main"), None).unwrap();
        assert_eq!(d.files.len(), 4);
        assert!(d
            .files
            .iter()
            .any(|f| f.path == "c.txt" && f.status == FileStatus::Added));
        assert!(diff_sync(path, Some("--all"), None).is_err());
        assert!(diff_sync(path, Some("nope"), None).is_err());

        sh(&repo, &["add", "-A"]);
        sh(&repo, &["commit", "-q", "-m", "four"]);
        sh(&repo, &["mv", "c.txt", "d.txt"]);
        let d = diff_sync(path, None, None).unwrap();
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
        let d = diff_sync(path, None, None).unwrap();
        assert!(d.base.is_none() && d.files.len() == 1 && d.files[0].untracked);
        let _ = std::fs::remove_dir_all(base);
    }

    fn numbered(n: usize, edits: &[(usize, &str)]) -> String {
        (1..=n)
            .map(|i| {
                edits
                    .iter()
                    .find(|(l, _)| *l == i)
                    .map_or(format!("line {i}"), |(_, t)| t.to_string())
                    + "\n"
            })
            .collect()
    }

    #[test]
    fn stages_files_and_single_hunks_then_commits() {
        let (base, repo) = repo("stage");
        let path = repo.to_str().unwrap();
        std::fs::write(repo.join("f.txt"), numbered(30, &[])).unwrap();
        sh(&repo, &["add", "."]);
        sh(&repo, &["commit", "-q", "-m", "f"]);

        // Two hunks far apart, plus a new file.
        std::fs::write(repo.join("f.txt"), numbered(30, &[(2, "TWO"), (28, "TWENTY-EIGHT")])).unwrap();
        std::fs::write(repo.join("new.txt"), "n\n").unwrap();
        let d = diff_sync(path, None, None).unwrap();
        assert!(d.staging && d.files.iter().all(|f| !f.staged));
        let f = d.files.iter().find(|f| f.path == "f.txt").unwrap();
        assert_eq!(f.hunks.len(), 2);

        // Stage only the second hunk: the file shows up in both parts.
        stage_hunk_sync(path, "f.txt", 1, &f.hunks[1].header, true).unwrap();
        let d = diff_sync(path, None, None).unwrap();
        let part = |staged| d.files.iter().find(|f| f.path == "f.txt" && f.staged == staged).unwrap();
        assert!(part(true).hunks.len() == 1 && part(true).hunks[0].lines.iter().any(|l| l.text == "TWENTY-EIGHT"));
        assert!(part(false).hunks.len() == 1 && part(false).hunks[0].lines.iter().any(|l| l.text == "TWO"));
        assert_eq!(status_sync(path).unwrap().staged, 1);

        // A stale hunk is refused, and so is a hunk of a new file.
        assert!(stage_hunk_sync(path, "f.txt", 0, "@@ -9,9 +9,9 @@", true).unwrap_err().contains("changed"));
        assert!(stage_hunk_sync(path, "new.txt", 0, "@@ -0,0 +1 @@", true).is_err());

        // Unstage that hunk again, then stage the whole file and the new one.
        let staged = part(true).hunks[0].header.clone();
        stage_hunk_sync(path, "f.txt", 0, &staged, false).unwrap();
        assert_eq!(status_sync(path).unwrap().staged, 0);
        stage_sync(path, &["f.txt".into(), "new.txt".into()], true).unwrap();
        let d = diff_sync(path, None, None).unwrap();
        assert!(d.files.iter().all(|f| f.staged) && d.files.len() == 2);
        stage_sync(path, &["new.txt".into()], false).unwrap();
        assert!(diff_sync(path, None, None).unwrap().files.iter().any(|f| f.path == "new.txt" && f.untracked));

        // Commit what is staged; the rest stays.
        assert!(commit_sync(path, "  ").unwrap_err().contains("message"));
        sh(&repo, &["config", "user.name", "t"]);
        sh(&repo, &["config", "user.email", "t@t"]);
        let c = commit_sync(path, "Edit f\n\nBody").unwrap();
        assert_eq!(c.subject, "Edit f");
        assert!(commit_sync(path, "again").unwrap_err().contains("Nothing is staged"));
        let d = diff_sync(path, None, None).unwrap();
        assert_eq!(d.files.len(), 1);
        assert!(d.files[0].untracked);

        // The commit's own diff.
        let d = diff_sync(path, None, Some(&c.sha)).unwrap();
        assert!(!d.staging && d.files.len() == 1 && d.files[0].hunks.len() == 2);
        assert!(diff_sync(path, None, Some("--all")).is_err());
        assert!(diff_sync(path, None, Some("abcdef0")).is_err());
        // The first commit is shown against nothing.
        let first = log_sync(path, None, true, 0, 50).unwrap().commits.pop().unwrap();
        assert_eq!(diff_sync(path, None, Some(&first.sha)).unwrap().files[0].path, "a.txt");
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn stages_in_a_repo_with_no_commits() {
        let base = std::env::temp_dir().join(format!("bach-git-stage-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        sh(&base, &["init", "-q", "-b", "main"]);
        let path = base.to_str().unwrap();
        std::fs::write(base.join("a.txt"), "a\n").unwrap();
        stage_sync(path, &["a.txt".into()], true).unwrap();
        assert_eq!(status_sync(path).unwrap().staged, 1);
        stage_sync(path, &["a.txt".into()], false).unwrap();
        assert_eq!(status_sync(path).unwrap().staged, 0);
        assert!(log_sync(path, None, true, 0, 50).unwrap().commits.is_empty());
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn lists_the_branch_commits_then_older_ones() {
        let (base, repo) = repo("log");
        let path = repo.to_str().unwrap();
        sh(&repo, &["switch", "-q", "-c", "feature"]);
        for n in 1..=3 {
            std::fs::write(repo.join(format!("{n}.txt")), "x").unwrap();
            sh(&repo, &["add", "."]);
            sh(&repo, &["commit", "-q", "-m", &format!("feature {n}")]);
        }
        let own = log_sync(path, Some("main"), false, 0, 2).unwrap();
        assert_eq!(own.commits.len(), 2);
        assert!(own.has_more && own.commits[0].subject == "feature 3");
        let rest = log_sync(path, Some("main"), false, 2, 2).unwrap();
        assert!(!rest.has_more && rest.commits.len() == 1);

        let all = log_sync(path, Some("main"), true, 0, 50).unwrap();
        let flags: Vec<_> = all.commits.iter().map(|c| c.on_branch).collect();
        assert_eq!(flags, [true, true, true, false]);
        assert!(log_sync(path, Some("nope"), false, 0, 1).is_err());
        assert!(log_sync(path, None, false, 0, 50).unwrap().commits.iter().all(|c| c.on_branch));
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn pushes_with_upstream_and_reports_problems() {
        let (base, repo) = repo("push");
        let path = repo.to_str().unwrap();
        sh(&repo, &["switch", "-q", "-c", "feature"]);

        // No remote at all.
        let err = push_sync(path).unwrap_err();
        assert!(err.contains("no remote"), "{err}");
        assert!(!status_sync(path).unwrap().has_remote);

        let remote = base.join("remote.git");
        sh(&base, &["init", "-q", "--bare", "-b", "main", remote.to_str().unwrap()]);
        sh(&repo, &["remote", "add", "origin", remote.to_str().unwrap()]);
        let s = status_sync(path).unwrap();
        assert!(s.has_remote && s.upstream.is_none() && s.ahead == 1);

        // First push sets the upstream.
        let s = push_sync(path).unwrap();
        assert_eq!((s.upstream.as_deref(), s.ahead, s.behind), (Some("origin/feature"), 0, 0));

        std::fs::write(repo.join("b.txt"), "b").unwrap();
        sh(&repo, &["add", "."]);
        sh(&repo, &["commit", "-q", "-m", "two"]);
        assert_eq!(status_sync(path).unwrap().ahead, 1);
        let s = push_sync(path).unwrap();
        assert_eq!((s.ahead, s.behind), (0, 0));

        // Someone else pushes to the branch: ours is rejected, and nothing is forced.
        let other = base.join("other");
        sh(&base, &["clone", "-q", "-b", "feature", remote.to_str().unwrap(), other.to_str().unwrap()]);
        std::fs::write(other.join("o.txt"), "o").unwrap();
        sh(&other, &["add", "."]);
        sh(&other, &["commit", "-q", "-m", "theirs"]);
        sh(&other, &["push", "-q"]);
        std::fs::write(repo.join("c.txt"), "c").unwrap();
        sh(&repo, &["add", "."]);
        sh(&repo, &["commit", "-q", "-m", "three"]);
        let err = push_sync(path).unwrap_err();
        assert!(err.starts_with("Rejected"), "{err}");
        sh(&repo, &["fetch", "-q"]);
        let s = status_sync(path).unwrap();
        assert_eq!((s.ahead, s.behind), (1, 1));

        // An unreachable remote.
        sh(&repo, &["remote", "set-url", "origin", base.join("missing.git").to_str().unwrap()]);
        let err = push_sync(path).unwrap_err();
        assert!(err.contains("remote") || err.contains("Couldn't"), "{err}");
        let _ = std::fs::remove_dir_all(base);
    }
}
