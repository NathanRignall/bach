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
}

/// The changes in a checkout, file by file.
#[derive(Debug, Default, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct GitDiff {
    /// The commit the working tree is compared with (abbreviated), or none before the first commit.
    pub base: Option<String>,
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
}
