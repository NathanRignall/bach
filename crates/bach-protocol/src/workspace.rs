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
