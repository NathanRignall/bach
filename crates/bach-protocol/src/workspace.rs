//! Folders and git on the backend host (where agents run).
use serde::Serialize;
use ts_rs::TS;

#[derive(Debug, Serialize, TS)]
pub struct DirEntry {
    pub name: String,
    pub path: String,
    /// Contains a `.git`, i.e. probably a project root.
    pub git: bool,
}

#[derive(Debug, Serialize, TS)]
pub struct DirListing {
    pub path: String,
    pub parent: Option<String>,
    pub home: String,
    pub entries: Vec<DirEntry>,
}

#[derive(Debug, Default, Serialize, TS)]
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

/// Where a session's agent runs.
#[derive(Debug, Serialize, TS)]
pub struct Workspace {
    /// The directory the agent should run in.
    pub workdir: String,
    pub branch: Option<String>,
    pub worktree: bool,
}

/// A worktree Bach created.
#[derive(Debug, Serialize, TS)]
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
    /// The branch's changes are already in the repository's base branch (main, master or the
    /// remote's default), whether it was merged, squashed or rebased.
    pub merged: bool,
}

/// The changes in a checkout, file by file.
#[derive(Debug, Default, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct GitDiff {
    /// The commit the changes are compared with (abbreviated), or none before the first commit.
    /// For a single commit's diff: that commit.
    pub base: Option<String>,
    /// Uncommitted changes: a file with both staged and unstaged changes is listed twice (see
    /// `FileDiff::staged`), and files and hunks can be staged and unstaged.
    pub staging: bool,
    pub files: Vec<FileDiff>,
    /// Some files' lines were left out to keep the diff a reasonable size.
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
}

#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FileDiff {
    pub path: String,
    /// Where a renamed file was.
    pub old_path: Option<String>,
    pub status: FileStatus,
    /// Not yet tracked by git.
    pub untracked: bool,
    /// In an uncommitted diff: the part of the file's changes that is staged (index against
    /// HEAD), as opposed to the unstaged part (working tree against index).
    pub staged: bool,
    pub binary: bool,
    pub additions: u32,
    pub deletions: u32,
    /// Empty for binary files, and for files too large to show (`omitted`).
    pub hunks: Vec<DiffHunk>,
    pub omitted: bool,
}

#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DiffHunk {
    /// The `@@ -a,b +c,d @@ context` line.
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum LineKind {
    Context,
    Add,
    Delete,
}

#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct DiffLine {
    pub kind: LineKind,
    pub text: String,
    /// Line number in the old file (none for added lines).
    pub old: Option<u32>,
    /// Line number in the new file (none for deleted lines).
    pub new: Option<u32>,
    /// The file doesn't end with a newline after this line.
    pub no_newline: bool,
}

/// The files in a session's folder, for the file browser.
#[derive(Debug, Default, Serialize, TS)]
pub struct FileList {
    /// Paths relative to the folder, `/`-separated, sorted.
    pub files: Vec<String>,
    /// There were more files than are listed.
    pub truncated: bool,
}

/// A file's bytes, for previewing images and PDFs and for downloads.
#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FileData {
    /// Guessed from the extension; `application/octet-stream` when unknown.
    pub mime: String,
    /// The contents, base64-encoded.
    pub data: String,
}

/// A file in a session's folder, for the file viewer.
#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FileContent {
    /// Relative to the session's folder.
    pub path: String,
    /// Size in bytes.
    pub size: u64,
    /// The file's text; none for binary files and files too large to show.
    pub text: Option<String>,
    pub binary: bool,
    /// Identifies the contents that were read (a hash of them; empty without `text`), to tell
    /// `write_file` what the edit started from.
    pub version: String,
    /// `text` can be edited and written back: it's all valid UTF-8 (else it was read lossily).
    pub editable: bool,
}

/// A commit, for the history list.
#[derive(Debug, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CommitInfo {
    pub sha: String,
    /// Abbreviated.
    pub short: String,
    pub subject: String,
    pub author: String,
    /// Unix time in seconds.
    pub time: i64,
    /// On the session's branch, as opposed to from before it left its base branch.
    pub on_branch: bool,
}

/// A page of the commits reachable from HEAD, newest first.
#[derive(Debug, Default, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct GitLog {
    pub commits: Vec<CommitInfo>,
    /// There are more commits after these.
    pub has_more: bool,
}

/// Where a branch stands against the remote.
#[derive(Debug, Default, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BranchStatus {
    /// None when HEAD is detached.
    pub branch: Option<String>,
    /// HEAD's commit (abbreviated); none before the first commit.
    pub head: Option<String>,
    /// The remote-tracking branch it pushes to, like `origin/feature`; none until first pushed.
    pub upstream: Option<String>,
    /// The repository has at least one remote.
    pub has_remote: bool,
    /// Commits the upstream lacks. Without an upstream: commits no remote branch has.
    pub ahead: u32,
    /// Commits the upstream has that the branch lacks (as of the last fetch).
    pub behind: u32,
    /// Files with staged changes.
    pub staged: u32,
}
