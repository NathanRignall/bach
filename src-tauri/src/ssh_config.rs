//! The hosts named in the user's `~/.ssh/config`, for the connection settings' host picker. Read
//! here, in the app, because it's the app's machine whose ssh setup `ssh <host>` will use.
use std::path::{Path, PathBuf};

/// Includes nested deeper than this are ignored (ssh itself stops at 16).
const MAX_DEPTH: usize = 8;

/// Host aliases from `~/.ssh/config` and the files it `Include`s, in order of appearance. Wildcard
/// patterns and negations (`*`, `?`, `!host`) aren't hosts you can connect to, so they're skipped.
#[tauri::command]
pub fn ssh_hosts() -> Vec<String> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return vec![];
    };
    let mut hosts = vec![];
    read(
        &home.join(".ssh/config"),
        &home.join(".ssh"),
        &home,
        0,
        &mut hosts,
    );
    hosts
}

fn read(file: &Path, ssh_dir: &Path, home: &Path, depth: usize, hosts: &mut Vec<String>) {
    let Ok(text) = std::fs::read_to_string(file) else {
        return;
    };
    for line in text.lines() {
        let Some((keyword, args)) = split_line(line) else {
            continue;
        };
        if keyword.eq_ignore_ascii_case("host") {
            for h in args {
                if !h.contains(['*', '?', '!']) && !hosts.contains(&h) {
                    hosts.push(h);
                }
            }
        } else if keyword.eq_ignore_ascii_case("include") && depth < MAX_DEPTH {
            for pattern in args {
                for included in expand(&pattern, ssh_dir, home) {
                    read(&included, ssh_dir, home, depth + 1, hosts);
                }
            }
        }
    }
}

/// `Keyword args…` or `Keyword=args…`, without comments; quoted arguments keep their spaces.
fn split_line(line: &str) -> Option<(&str, Vec<String>)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let end = line.find(|c: char| c.is_whitespace() || c == '=')?;
    let (keyword, rest) = line.split_at(end);
    let rest = rest.trim_start().trim_start_matches('=');
    let mut args = vec![];
    let mut cur = String::new();
    let mut quoted = false;
    let mut any = false;
    for c in rest.chars() {
        match c {
            '"' => (quoted, any) = (!quoted, true),
            '#' if !quoted => break,
            c if c.is_whitespace() && !quoted => {
                if any {
                    args.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => (cur.push(c), any = true).1,
        }
    }
    if any {
        args.push(cur);
    }
    Some((keyword, args))
}

/// The files an `Include` pattern names: `~` is home, relative paths are under `~/.ssh`, and `*`
/// and `?` work in the file name (the usual `Include conf.d/*`).
fn expand(pattern: &str, ssh_dir: &Path, home: &Path) -> Vec<PathBuf> {
    let path = match pattern.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None => ssh_dir.join(pattern), // absolute paths replace the base
    };
    let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
        return vec![];
    };
    if !name.contains(['*', '?']) {
        return vec![path];
    }
    let Some(dir) = path.parent() else {
        return vec![];
    };
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|e| wildcard(&name, &e.file_name().to_string_lossy()))
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    files.sort();
    files
}

/// Whether `text` matches `pattern`, where `*` is any run of characters and `?` any one.
fn wildcard(pattern: &str, text: &str) -> bool {
    fn go(p: &[char], t: &[char]) -> bool {
        match p.split_first() {
            None => t.is_empty(),
            Some(('*', rest)) => (0..=t.len()).any(|i| go(rest, &t[i..])),
            Some((c, rest)) => t.first().is_some_and(|x| *c == '?' || c == x) && go(rest, &t[1..]),
        }
    }
    go(
        &pattern.chars().collect::<Vec<_>>(),
        &text.chars().collect::<Vec<_>>(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_hosts_through_includes_and_skips_patterns() {
        let home = std::env::temp_dir().join(format!("bach-sshcfg-{}", std::process::id()));
        let ssh = home.join(".ssh");
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(ssh.join("conf.d")).unwrap();
        std::fs::write(
            ssh.join("config"),
            "# mine\nInclude conf.d/*.conf\nInclude ~/extra\nhost orion  Orion2 # comment\n  HostName=10.0.0.1\nHost *\n  User x\nHost *.corp !bad dup\nHost=\"quoted\"\nHost dup\n",
        )
        .unwrap();
        std::fs::write(ssh.join("conf.d/a.conf"), "Host work-a\nInclude a.conf\n").unwrap();
        std::fs::write(ssh.join("conf.d/skip.txt"), "Host not-included\n").unwrap();
        std::fs::write(home.join("extra"), "Host  extra1 extra2\n").unwrap();

        let mut hosts = vec![];
        read(&ssh.join("config"), &ssh, &home, 0, &mut hosts);
        assert_eq!(
            hosts,
            ["work-a", "extra1", "extra2", "orion", "Orion2", "dup", "quoted"]
        );
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn wildcards_match_file_names() {
        assert!(wildcard("*.conf", "a.conf") && !wildcard("*.conf", "a.txt"));
        assert!(wildcard("a?c", "abc") && !wildcard("a?c", "ac"));
    }
}
