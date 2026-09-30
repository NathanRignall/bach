//! Directory browsing for the folder picker, and reading images agents link to. Runs on the
//! backend host, since that's where agents run (and so where project folders live).
pub use bach_protocol::{ApiError, DirEntry, DirListing};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use std::path::{Path, PathBuf};

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

#[cfg(test)]
mod tests {
    use super::*;

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
