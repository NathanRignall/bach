//! Directory browsing for the folder picker, reading images agents link to, and the file
//! browser's view of a session's folder. Runs on the backend host, since that's where agents run
//! (and so where project folders live).
pub use bach_protocol::{ApiError, DirEntry, DirListing, FileContent, FileList};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use std::{
    path::{Component, Path, PathBuf},
    process::Command,
};

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

const MAX_IMAGE_BYTES: u64 = 25 * 1024 * 1024;

/// Reads an image file as a `data:` URL. The path must be absolute (or start with `~`), and only
/// image files are read, since this lets the UI read files on the backend host.
pub fn read_image(path: &str) -> Result<String, ApiError> {
    let path = expand_home(path.trim());
    let shown = path.display();
    if !path.is_absolute() {
        return Err(ApiError::invalid(format!("{shown}: not an absolute path")));
    }
    let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
    let mime = match ext.as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("avif") => "image/avif",
        Some("bmp") => "image/bmp",
        Some("svg") => "image/svg+xml",
        _ => return Err(ApiError::invalid(format!("{shown}: not an image file"))),
    };
    let meta = std::fs::metadata(&path).map_err(|e| ApiError::not_found(format!("{shown}: {e}")))?;
    if !meta.is_file() {
        return Err(ApiError::invalid(format!("{shown}: not a file")));
    }
    if meta.len() > MAX_IMAGE_BYTES {
        return Err(ApiError::invalid(format!("{shown}: larger than 25 MB")));
    }
    let bytes = std::fs::read(&path).map_err(|e| ApiError::failed(format!("{shown}: {e}")))?;
    Ok(format!("data:{mime};base64,{}", B64.encode(bytes)))
}

const MAX_FILES: usize = 20_000;
const MAX_TEXT_BYTES: u64 = 1024 * 1024;

/// The session folder `root`, resolved (`~` expanded, symlinks followed).
fn session_root(root: &str) -> Result<PathBuf, ApiError> {
    let root = expand_home(root.trim());
    root.canonicalize()
        .map_err(|e| ApiError::not_found(format!("{}: {e}", root.display())))
}

/// The files in `root`: those git doesn't ignore or, outside a repository, every file but those
/// in `.git` folders. Relative, `/`-separated and sorted; at most [`MAX_FILES`].
pub fn list_files(root: &str) -> Result<FileList, ApiError> {
    let root = session_root(root)?;
    let mut files = match git_files(&root) {
        Some(files) => files,
        None => {
            let mut files = Vec::new();
            walk(&root, "", &mut files);
            files
        }
    };
    files.sort();
    files.dedup();
    let truncated = files.len() > MAX_FILES;
    files.truncate(MAX_FILES);
    Ok(FileList { files, truncated })
}

/// Tracked and untracked files git doesn't ignore, that are still there; none outside a repository.
fn git_files(root: &Path) -> Option<Vec<String>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["ls-files", "-z", "--cached", "--others", "--exclude-standard"])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    Some(
        out.stdout
            .split(|&b| b == 0)
            .filter(|p| !p.is_empty())
            .map(|p| String::from_utf8_lossy(p).into_owned())
            // Deleted (but still tracked) files are gone, and submodules are folders.
            .filter(|p| root.join(p).symlink_metadata().is_ok_and(|m| !m.is_dir()))
            .collect(),
    )
}

/// Every file under `dir` (symlinks included, not followed), until there are more than enough.
fn walk(dir: &Path, prefix: &str, files: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.filter_map(Result::ok) {
        if files.len() > MAX_FILES {
            return;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        let path = format!("{prefix}{name}");
        match e.file_type() {
            Ok(t) if t.is_dir() => {
                if name != ".git" {
                    walk(&e.path(), &format!("{path}/"), files);
                }
            }
            Ok(_) => files.push(path),
            Err(_) => {}
        }
    }
}

/// The file at `path` in the folder `root`. `path` must be relative and stay inside `root`, also
/// after following symlinks. Text files up to 1 MB come with their text.
pub fn read_file(root: &str, path: &str) -> Result<FileContent, ApiError> {
    let root = session_root(root)?;
    let rel = Path::new(path);
    let outside = || ApiError::invalid(format!("{path}: not a file in the session's folder"));
    if path.is_empty()
        || rel
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return Err(outside());
    }
    let full = root
        .join(rel)
        .canonicalize()
        .map_err(|e| ApiError::not_found(format!("{path}: {e}")))?;
    if !full.starts_with(&root) {
        return Err(outside());
    }
    let meta = std::fs::metadata(&full).map_err(|e| ApiError::not_found(format!("{path}: {e}")))?;
    if !meta.is_file() {
        return Err(ApiError::invalid(format!("{path}: not a file")));
    }
    let mut file = FileContent {
        path: path.to_string(),
        size: meta.len(),
        text: None,
        binary: false,
    };
    if meta.len() > MAX_TEXT_BYTES {
        return Ok(file);
    }
    let bytes = std::fs::read(&full).map_err(|e| ApiError::failed(format!("{path}: {e}")))?;
    // Like git: a NUL near the start means binary.
    file.binary = bytes.iter().take(8000).any(|&b| b == 0);
    if !file.binary {
        file.text = Some(String::from_utf8_lossy(&bytes).into_owned());
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("bach-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn lists_files_git_does_not_ignore() {
        let root = temp("files-git");
        let git = |args: &[&str]| {
            let ok = Command::new("git").arg("-C").arg(&root).args(args).status().unwrap();
            assert!(ok.success(), "git {args:?}");
        };
        git(&["init", "-q"]);
        for (p, text) in [
            (".gitignore", "target/\n"),
            ("src/main.rs", "fn main() {}"),
            ("target/debug/x", ""),
            ("gone.txt", ""),
            ("new.txt", ""),
        ] {
            std::fs::create_dir_all(root.join(p).parent().unwrap()).unwrap();
            std::fs::write(root.join(p), text).unwrap();
        }
        git(&["add", ".gitignore", "src", "gone.txt"]);
        std::fs::remove_file(root.join("gone.txt")).unwrap();

        let l = list_files(root.to_str().unwrap()).unwrap();
        assert_eq!(l.files, [".gitignore", "new.txt", "src/main.rs"]);
        assert!(!l.truncated);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn lists_every_file_outside_a_repository() {
        let root = temp("files-plain");
        for p in ["a/b.txt", ".git/HEAD", ".env", "c.txt"] {
            std::fs::create_dir_all(root.join(p).parent().unwrap()).unwrap();
            std::fs::write(root.join(p), "").unwrap();
        }
        let l = list_files(root.to_str().unwrap()).unwrap();
        assert_eq!(l.files, [".env", "a/b.txt", "c.txt"]);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn reads_files_only_inside_the_folder() {
        let base = temp("read");
        let root = base.join("proj");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.rs"), "fn a() {}\n").unwrap();
        std::fs::write(root.join("bin"), [0u8, 1, 2]).unwrap();
        std::fs::write(root.join("big"), vec![b'x'; MAX_TEXT_BYTES as usize + 1]).unwrap();
        std::fs::write(base.join("secret"), "s").unwrap();
        std::os::unix::fs::symlink(base.join("secret"), root.join("link-out")).unwrap();
        std::os::unix::fs::symlink(root.join("src/a.rs"), root.join("link-in")).unwrap();
        let r = root.to_str().unwrap();

        let f = read_file(r, "src/a.rs").unwrap();
        assert_eq!((f.text.as_deref(), f.size, f.binary), (Some("fn a() {}\n"), 10, false));
        assert_eq!(read_file(r, "./link-in").unwrap().text.as_deref(), Some("fn a() {}\n"));
        let bin = read_file(r, "bin").unwrap();
        assert!(bin.binary && bin.text.is_none());
        let big = read_file(r, "big").unwrap();
        assert!(!big.binary && big.text.is_none() && big.size == MAX_TEXT_BYTES + 1);

        let secret = base.join("secret");
        for bad in ["../secret", "link-out", secret.to_str().unwrap(), "src", "", "missing"] {
            assert!(read_file(r, bad).is_err(), "{bad}");
        }
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn reads_only_image_files_at_absolute_paths() {
        let root = std::env::temp_dir().join(format!("bach-img-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("dir.png")).unwrap();
        std::fs::write(root.join("a.PNG"), [1, 2, 3]).unwrap();
        std::fs::write(root.join("secret.txt"), "x").unwrap();

        assert_eq!(read_image(root.join("a.PNG").to_str().unwrap()).unwrap(), "data:image/png;base64,AQID");
        for bad in [root.join("secret.txt"), root.join("dir.png"), root.join("missing.png")] {
            assert!(read_image(bad.to_str().unwrap()).is_err(), "{}", bad.display());
        }
        assert!(read_image("relative.png").is_err());
        let _ = std::fs::remove_dir_all(root);
    }

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
