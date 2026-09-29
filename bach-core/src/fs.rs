//! Directory browsing for the folder picker. Runs on the backend host, since that's where
//! agents run (and so where project folders live).
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Serialize, Debug)]
pub struct DirEntry {
    pub name: String,
    pub path: String,
    /// Contains a `.git`, i.e. probably a project root.
    pub git: bool,
}

#[derive(Serialize, Debug)]
pub struct DirListing {
    pub path: String,
    pub parent: Option<String>,
    pub home: String,
    pub entries: Vec<DirEntry>,
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// Expands a leading `~` to the home directory.
pub fn expand_home(path: &str) -> PathBuf {
    match path {
        "~" => home(),
        p => match p.strip_prefix("~/") {
            Some(rest) => home().join(rest),
            None => PathBuf::from(p),
        },
    }
}

/// Lists the sub-directories of `path` (default: the home directory). `~` is expanded.
pub fn list_dir(path: Option<&str>, show_hidden: bool) -> Result<DirListing, String> {
    let home = home();
    let requested = match path.map(str::trim).filter(|p| !p.is_empty()) {
        None => home.clone(),
        Some(p) => expand_home(p),
    };
    let dir = requested
        .canonicalize()
        .map_err(|e| format!("{}: {e}", requested.display()))?;

    let mut entries: Vec<DirEntry> = std::fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if !show_hidden && name.starts_with('.') {
                return None;
            }
            let path = e.path();
            // metadata() follows symlinks, so links to directories are listed too.
            std::fs::metadata(&path).ok()?.is_dir().then(|| DirEntry {
                git: path.join(".git").exists(),
                path: path.to_string_lossy().into_owned(),
                name,
            })
        })
        .collect();
    entries.sort_by_key(|e| e.name.to_lowercase());

    Ok(DirListing {
        parent: dir
            .parent()
            .map(Path::to_string_lossy)
            .map(|p| p.into_owned()),
        path: dir.to_string_lossy().into_owned(),
        home: home.to_string_lossy().into_owned(),
        entries,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_only_directories_sorted_and_hides_dotfiles() {
        let root = std::env::temp_dir().join(format!("bach-fs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for d in ["Beta", "alpha/.git", ".hidden"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::write(root.join("file.txt"), "x").unwrap();

        let l = list_dir(root.to_str(), false).unwrap();
        let names: Vec<_> = l.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["alpha", "Beta"]);
        assert!(l.entries[0].git && !l.entries[1].git);
        assert_eq!(
            l.parent.as_deref(),
            root.canonicalize().unwrap().parent().unwrap().to_str()
        );

        let all = list_dir(root.to_str(), true).unwrap();
        assert_eq!(all.entries.len(), 3);
        assert!(list_dir(root.join("nope").to_str(), false).is_err());
        let _ = std::fs::remove_dir_all(root);
    }
}
