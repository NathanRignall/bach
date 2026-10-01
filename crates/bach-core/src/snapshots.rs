//! Where a session's worktree stood at the start of each turn, so a fork can begin from there.
//!
//! Each turn's state is a commit of the whole working tree, uncommitted and untracked files
//! included (but not what `.gitignore` leaves out), kept under a ref nobody checks out:
//! `refs/bach/snapshots/<session id>/<seq of the user message>`. Making one builds a tree in a
//! throwaway copy of the index, so the branch, the real index, the stash and the files themselves
//! are never touched. The commit's parent is what `HEAD` was, which is how a fork tells the
//! committed part from the rest.
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};

const REFS: &str = "refs/bach/snapshots";

fn git(dir: &Path, args: &[&str], envs: &[(&str, &Path)]) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir).args(args).stdin(Stdio::null());
    // A snapshot is made by Bach, whoever's git identity is (not) set up.
    for (k, v) in [
        ("GIT_AUTHOR_NAME", "Bach"),
        ("GIT_AUTHOR_EMAIL", "bach@localhost"),
        ("GIT_COMMITTER_NAME", "Bach"),
        ("GIT_COMMITTER_EMAIL", "bach@localhost"),
    ] {
        cmd.env(k, v);
    }
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let out = cmd.output().map_err(|e| format!("failed to run git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn ref_name(session_id: &str, seq: u64) -> String {
    format!("{REFS}/{session_id}/{seq}")
}

/// Records `dir`'s working tree as session `session_id`'s state before its user message `seq`.
/// `Ok(false)` when `dir` isn't a git repository with a commit to build on: nothing to record.
pub fn take(dir: &Path, session_id: &str, seq: u64) -> Result<bool, String> {
    let Ok(head) = git(dir, &["rev-parse", "--verify", "--quiet", "HEAD^{commit}"], &[]) else {
        return Ok(false);
    };
    // Start from the real index (so unchanged files aren't read again) in a copy that is
    // ours to change, then let `add -A` make it match the working tree.
    static N: AtomicU64 = AtomicU64::new(0);
    let real = PathBuf::from(git(dir, &["rev-parse", "--path-format=absolute", "--git-path", "index"], &[])?);
    let temp = real.with_file_name(format!("bach-snapshot-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
    let made = (|| {
        if std::fs::copy(&real, &temp).is_err() {
            git(dir, &["read-tree", "HEAD"], &[("GIT_INDEX_FILE", &temp)])?;
        }
        let env = [("GIT_INDEX_FILE", temp.as_path())];
        git(dir, &["add", "-A", "--", "."], &env)?;
        let tree = git(dir, &["write-tree"], &env)?;
        git(dir, &["commit-tree", &tree, "-p", &head, "-m", &format!("Bach snapshot of session {session_id} before message {seq}")], &[])
    })();
    let _ = std::fs::remove_file(&temp);
    git(dir, &["update-ref", &ref_name(session_id, seq), &made?], &[])?;
    Ok(true)
}

/// The recorded snapshot commit, if there is one.
pub fn find(dir: &Path, session_id: &str, seq: u64) -> Option<String> {
    git(dir, &["rev-parse", "--verify", "--quiet", &format!("{}^{{commit}}", ref_name(session_id, seq))], &[]).ok()
}

/// Lets session `to` have the snapshots `from` has for the user messages before `before`, so the
/// copied part of a fork's transcript can be forked from in turn.
pub fn copy(dir: &Path, from: &str, to: &str, before: u64) {
    let Ok(list) = git(dir, &["for-each-ref", "--format=%(refname) %(objectname)", &format!("{REFS}/{from}/")], &[]) else {
        return;
    };
    for line in list.lines() {
        let Some((name, commit)) = line.split_once(' ') else { continue };
        let Some(seq) = name.rsplit('/').next().and_then(|s| s.parse::<u64>().ok()) else { continue };
        if seq < before {
            let _ = git(dir, &["update-ref", &ref_name(to, seq), commit], &[]);
        }
    }
}

/// Forgets session `id`'s snapshots.
pub fn remove(dir: &Path, id: &str) {
    let Ok(list) = git(dir, &["for-each-ref", "--format=%(refname)", &format!("{REFS}/{id}/")], &[]) else {
        return;
    };
    for name in list.lines() {
        let _ = git(dir, &["update-ref", "-d", name], &[]);
    }
}

/// Makes the fresh worktree `dir` (on a new branch nobody has used) look like the snapshot
/// `commit`: its branch starts where the snapshot's `HEAD` was, and the files are as they were,
/// with what was uncommitted uncommitted and unstaged, and what was untracked untracked.
pub fn restore(dir: &Path, commit: &str) -> Result<(), String> {
    let parent = git(dir, &["rev-parse", &format!("{commit}^")], &[])?;
    git(dir, &["reset", "--hard", "--quiet", &parent], &[])?;
    // Bring the files to the snapshot's (deleting what it had deleted), then leave git seeing
    // them as changes from `HEAD` rather than as staged.
    git(dir, &["read-tree", "--reset", "-u", commit], &[])?;
    git(dir, &["reset", "--quiet"], &[])?;
    Ok(())
}

/// What the work in `dir` has changed, relative to the repository's root: paths that differ from
/// the merge base with `base` (the branch it was started from), plus untracked files. Sorted.
pub fn changed_files(dir: &Path, base: Option<&str>) -> Vec<String> {
    let since = base
        .and_then(|b| git(dir, &["merge-base", "--end-of-options", b, "HEAD"], &[]).ok())
        .unwrap_or_else(|| "HEAD".into());
    let mut files: Vec<String> = git(dir, &["-c", "core.quotePath=false", "diff", "--name-only", &since], &[])
        .into_iter()
        .chain(git(dir, &["-c", "core.quotePath=false", "ls-files", "--others", "--exclude-standard", "--full-name"], &[]))
        .flat_map(|l| l.lines().map(str::to_string).collect::<Vec<_>>())
        .collect();
    files.sort();
    files.dedup();
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(dir: &Path, args: &[&str]) -> String {
        git(dir, args, &[]).unwrap_or_else(|e| panic!("git {args:?}: {e}"))
    }

    fn repo(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bach-snap-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        sh(&dir, &["init", "-q", "-b", "main"]);
        sh(&dir, &["config", "user.email", "t@t"]);
        sh(&dir, &["config", "user.name", "t"]);
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        std::fs::write(dir.join("gone.txt"), "bye\n").unwrap();
        std::fs::write(dir.join(".gitignore"), "ignored.log\n").unwrap();
        sh(&dir, &["add", "."]);
        sh(&dir, &["commit", "-qm", "init"]);
        dir
    }

    #[test]
    fn snapshots_everything_without_touching_the_repository() {
        let dir = repo("take");
        // Staged, unstaged, untracked, deleted and ignored.
        std::fs::write(dir.join("a.txt"), "two\n").unwrap();
        std::fs::write(dir.join("staged.txt"), "s\n").unwrap();
        sh(&dir, &["add", "staged.txt"]);
        std::fs::write(dir.join("new.txt"), "n\n").unwrap();
        std::fs::remove_file(dir.join("gone.txt")).unwrap();
        std::fs::write(dir.join("ignored.log"), "x\n").unwrap();
        sh(&dir, &["stash", "list"]);
        let (status, index, branches) = (
            sh(&dir, &["status", "--porcelain"]),
            sh(&dir, &["ls-files", "--stage"]),
            sh(&dir, &["for-each-ref", "--format=%(refname)", "refs/heads"]),
        );

        assert!(take(&dir, "s1", 4).unwrap());

        assert_eq!(sh(&dir, &["status", "--porcelain"]), status, "the working tree is as it was");
        assert_eq!(sh(&dir, &["ls-files", "--stage"]), index, "so is the index");
        assert_eq!(sh(&dir, &["for-each-ref", "--format=%(refname)", "refs/heads"]), branches);
        assert_eq!(sh(&dir, &["stash", "list"]), "");
        let snap = find(&dir, "s1", 4).unwrap();
        let files = sh(&dir, &["ls-tree", "-r", "--name-only", &snap]);
        assert_eq!(files.lines().collect::<Vec<_>>(), [".gitignore", "a.txt", "new.txt", "staged.txt"]);
        assert_eq!(sh(&dir, &["show", &format!("{snap}:a.txt")]), "two");
        assert_eq!(sh(&dir, &["rev-parse", &format!("{snap}^")]), sh(&dir, &["rev-parse", "HEAD"]));
        assert!(find(&dir, "s1", 5).is_none());
        // No temporary index left behind.
        assert!(std::fs::read_dir(dir.join(".git")).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().starts_with("bach-snapshot")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn restores_into_a_fresh_worktree_and_copies_and_removes_refs() {
        let dir = repo("restore");
        // The session committed something, then left more uncommitted.
        std::fs::write(dir.join("a.txt"), "two\n").unwrap();
        sh(&dir, &["commit", "-qam", "agent commit"]);
        std::fs::write(dir.join("a.txt"), "three\n").unwrap();
        std::fs::write(dir.join("new.txt"), "n\n").unwrap();
        std::fs::remove_file(dir.join("gone.txt")).unwrap();
        take(&dir, "s1", 2).unwrap();
        std::fs::write(dir.join("a.txt"), "later\n").unwrap();
        take(&dir, "s1", 7).unwrap();

        copy(&dir, "s1", "s2", 7);
        assert!(find(&dir, "s2", 2).is_some() && find(&dir, "s2", 7).is_none());

        let fork = dir.with_extension("fork");
        let _ = std::fs::remove_dir_all(&fork);
        sh(&dir, &["worktree", "add", "-q", "-b", "fork", &fork.to_string_lossy(), "main"]);
        restore(&fork, &find(&dir, "s1", 2).unwrap()).unwrap();
        assert_eq!(std::fs::read_to_string(fork.join("a.txt")).unwrap(), "three\n");
        assert_eq!(std::fs::read_to_string(fork.join("new.txt")).unwrap(), "n\n");
        assert!(!fork.join("gone.txt").exists());
        // Committed work is in the branch, the rest is uncommitted and unstaged.
        assert_eq!(sh(&fork, &["log", "--format=%s"]).lines().next(), Some("agent commit"));
        // (`sh` trims the first line's leading space.)
        assert_eq!(sh(&fork, &["status", "--porcelain"]), "M a.txt\n D gone.txt\n?? new.txt");
        assert_eq!(changed_files(&fork, Some("main")), ["a.txt", "gone.txt", "new.txt"]);

        remove(&dir, "s1");
        assert!(find(&dir, "s1", 2).is_none() && find(&dir, "s2", 2).is_some());
        sh(&dir, &["worktree", "remove", "--force", &fork.to_string_lossy()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_without_commits_has_nothing_to_snapshot() {
        let dir = std::env::temp_dir().join(format!("bach-snap-none-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!take(&dir, "s", 1).unwrap());
        sh(&dir, &["init", "-q"]);
        assert!(!take(&dir, "s", 1).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
