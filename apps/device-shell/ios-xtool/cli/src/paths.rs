//! Paths: where this tool and xtool keep things, Windows paths typed into
//! WSL, and where a key may (not) be kept.

use std::path::{Component, Path, PathBuf};

/// The directories the tool reads and writes. Tests point them at a
/// temporary directory.
#[derive(Clone, Debug)]
pub struct Paths {
    pub home: PathBuf,
    /// `~/.config/limiara-sideload`: the state file and `keys/`.
    pub config: PathBuf,
    /// `~/.config/xtool`, xtool's persistent directory.
    pub xtool_config: PathBuf,
    /// `~/.local`.
    pub local: PathBuf,
    /// Where Windows' user profiles are mounted (`/mnt/c/Users`).
    pub windows_users: PathBuf,
}

impl Paths {
    pub fn from_home(home: PathBuf) -> Paths {
        Paths {
            config: home.join(".config/limiara-sideload"),
            xtool_config: home.join(".config/xtool"),
            local: home.join(".local"),
            windows_users: PathBuf::from("/mnt/c/Users"),
            home,
        }
    }

    pub fn detect() -> Option<Paths> {
        let home = std::env::var_os("HOME").filter(|h| !h.is_empty())?;
        Some(Paths::from_home(PathBuf::from(home)))
    }

    pub fn state_file(&self) -> PathBuf {
        self.config.join("state.toml")
    }

    pub fn keys_dir(&self) -> PathBuf {
        self.config.join("keys")
    }

    pub fn xtool_data(&self) -> PathBuf {
        self.xtool_config.join("data")
    }

    /// The file xtool keeps its login in (`DirectoryStorage`, key
    /// `XTLAuthToken`). In API-key mode it holds the key itself.
    pub fn xtool_token(&self) -> PathBuf {
        self.xtool_data().join("XTLAuthToken")
    }

    pub fn local_bin(&self) -> PathBuf {
        self.local.join("bin")
    }

    pub fn swiftly_bin(&self) -> PathBuf {
        self.local.join("share/swiftly/bin")
    }

    pub fn cargo_bin(&self) -> PathBuf {
        self.home.join(".cargo/bin")
    }

    /// The directories that tools installed here live in, to put first on
    /// the children's `PATH`.
    pub fn tool_dirs(&self) -> Vec<PathBuf> {
        vec![self.local_bin(), self.swiftly_bin(), self.cargo_bin()]
    }

    /// Windows profiles' Downloads folders, skipping the shared profiles.
    pub fn windows_downloads(&self) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(&self.windows_users) else {
            return Vec::new();
        };
        let mut dirs: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                !matches!(
                    name.as_ref(),
                    "Public" | "Default" | "Default User" | "All Users" | "desktop.ini"
                )
            })
            .map(|entry| entry.path().join("Downloads"))
            .filter(|path| path.is_dir())
            .collect();
        dirs.sort();
        dirs
    }
}

/// Files in `dirs` (not recursive) whose name `matches`, newest first.
pub fn find_files(dirs: &[PathBuf], matches: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let name = entry.file_name();
            if !matches(&name.to_string_lossy()) {
                continue;
            }
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_file() {
                let modified = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
                found.push((modified, entry.path()));
            }
        }
    }
    found.sort_by_key(|entry| std::cmp::Reverse(entry.0));
    found.into_iter().map(|(_, path)| path).collect()
}

pub fn is_xcode_xip(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with("xcode") && lower.ends_with(".xip")
}

pub fn is_asc_key(name: &str) -> bool {
    name.starts_with("AuthKey_") && name.ends_with(".p8")
}

/// The key ID in an `AuthKey_<KEYID>.p8` file name.
pub fn key_id_from_file_name(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    let id = name.strip_prefix("AuthKey_")?.strip_suffix(".p8")?;
    crate::validate::key_id(id).ok()
}

/// Turns a Windows path into a WSL path (`wslpath -u`).
pub type PathConverter<'a> = &'a dyn Fn(&str) -> Option<String>;

/// A path as the user typed or pasted it, as a Linux path: quotes taken
/// off, `~` expanded, and a Windows path (`C:\Users\…`,
/// `\\wsl.localhost\Ubuntu\home\…`) turned into its WSL path. `wslpath`
/// is asked first when it is given; the rules here are its defaults.
pub fn normalize_user_path(
    input: &str,
    home: &Path,
    wslpath: Option<PathConverter<'_>>,
) -> Option<PathBuf> {
    let mut value = input.trim();
    for quote in ['"', '\''] {
        if value.len() >= 2 && value.starts_with(quote) && value.ends_with(quote) {
            value = &value[1..value.len() - 1];
        }
    }
    if value.is_empty() {
        return None;
    }
    if value == "~" {
        return Some(home.to_path_buf());
    }
    if let Some(rest) = value.strip_prefix("~/") {
        return Some(home.join(rest));
    }
    let windows = is_windows_path(value);
    if windows {
        if let Some(convert) = wslpath {
            if let Some(converted) = convert(value) {
                return Some(PathBuf::from(converted));
            }
        }
        return windows_to_wsl(value);
    }
    Some(PathBuf::from(value))
}

fn is_windows_path(value: &str) -> bool {
    let bytes = value.as_bytes();
    let drive = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/');
    drive || value.starts_with("\\\\")
}

/// `C:\Users\Ada\x` → `/mnt/c/Users/Ada/x`;
/// `\\wsl$\Ubuntu\home\ada\x` and `\\wsl.localhost\Ubuntu\home\ada\x` →
/// `/home/ada/x`.
pub fn windows_to_wsl(value: &str) -> Option<PathBuf> {
    let bytes = value.as_bytes();
    if bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        let drive = (bytes[0] as char).to_ascii_lowercase();
        let rest = value[2..].replace('\\', "/");
        let rest = rest.trim_start_matches('/');
        let mut path = PathBuf::from(format!("/mnt/{drive}"));
        if !rest.is_empty() {
            path.push(rest);
        }
        return Some(path);
    }
    let unc = value.strip_prefix("\\\\")?.replace('\\', "/");
    let mut parts = unc.splitn(3, '/');
    let host = parts.next()?.to_ascii_lowercase();
    let _distro = parts.next()?;
    let rest = parts.next().unwrap_or("");
    if host == "wsl$" || host == "wsl.localhost" {
        Some(PathBuf::from(format!("/{rest}")))
    } else {
        None
    }
}

/// Whether `path` is on a Windows drive mounted into WSL (`/mnt/<x>/…`).
pub fn is_windows_mount(path: &Path) -> bool {
    let mut components = path.components();
    matches!(components.next(), Some(Component::RootDir))
        && matches!(components.next(), Some(Component::Normal(name)) if name == "mnt")
        && components.next().is_some()
}

/// The root of the git working tree holding `path`, if any: the nearest
/// ancestor with a `.git` directory or file.
pub fn git_work_tree(path: &Path) -> Option<PathBuf> {
    let mut current = if path.is_dir() {
        Some(path)
    } else {
        path.parent()
    };
    while let Some(dir) = current {
        if dir.join(".git").exists() {
            return Some(dir.to_path_buf());
        }
        current = dir.parent();
    }
    None
}

/// Whether a key may be kept at `path`: not on a Windows drive and not in
/// any git working tree.
pub fn check_key_destination(path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("the location must be an absolute path".into());
    }
    if is_windows_mount(path) {
        return Err(
            "it is on a Windows drive (/mnt), which Windows apps, sync tools and backups can read"
                .into(),
        );
    }
    if let Some(tree) = git_work_tree(path) {
        return Err(format!(
            "it is inside the git working tree {}, where it could be committed",
            tree.display()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_paths_become_wsl_paths() {
        let home = Path::new("/home/ada");
        let cases = [
            (
                r"C:\Users\Ada\Downloads\AuthKey_ABC.p8",
                "/mnt/c/Users/Ada/Downloads/AuthKey_ABC.p8",
            ),
            (
                r#""D:\Xcode 27.1\Xcode_27.1.xip""#,
                "/mnt/d/Xcode 27.1/Xcode_27.1.xip",
            ),
            ("c:/Users/Ada/x.xip", "/mnt/c/Users/Ada/x.xip"),
            (r"\\wsl$\Ubuntu-24.04\home\ada\k.p8", "/home/ada/k.p8"),
            (
                r"\\wsl.localhost\Ubuntu\home\ada\Downloads\x.xip",
                "/home/ada/Downloads/x.xip",
            ),
            ("~/Downloads/x.xip", "/home/ada/Downloads/x.xip"),
            ("'/tmp/a b'", "/tmp/a b"),
            ("/home/ada/k.p8", "/home/ada/k.p8"),
        ];
        for (input, expected) in cases {
            assert_eq!(
                normalize_user_path(input, home, None),
                Some(PathBuf::from(expected)),
                "{input}"
            );
        }
        assert_eq!(normalize_user_path("  ", home, None), None);
        assert_eq!(windows_to_wsl(r"\\server\share\x"), None);
    }

    #[test]
    fn wslpath_is_asked_first() {
        let home = Path::new("/home/ada");
        let convert = |_: &str| Some("/custom/c/x".to_string());
        assert_eq!(
            normalize_user_path(r"C:\x", home, Some(&convert)),
            Some(PathBuf::from("/custom/c/x"))
        );
        // A Linux path is never handed to wslpath.
        assert_eq!(
            normalize_user_path("/home/ada/x", home, Some(&convert)),
            Some(PathBuf::from("/home/ada/x"))
        );
    }

    #[test]
    fn windows_mounts() {
        assert!(is_windows_mount(Path::new("/mnt/c/Users/x")));
        assert!(!is_windows_mount(Path::new("/mnt")));
        assert!(!is_windows_mount(Path::new("/home/mnt/c")));
    }

    #[test]
    fn keys_are_refused_in_git_trees_and_on_windows_drives() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(repo.join("sub")).unwrap();
        let inside = repo.join("sub/AuthKey_X.p8");
        let err = check_key_destination(&inside).unwrap_err();
        assert!(err.contains("git working tree"));
        // A worktree's `.git` is a file.
        let worktree = dir.path().join("wt");
        std::fs::create_dir_all(&worktree).unwrap();
        std::fs::write(worktree.join(".git"), "gitdir: elsewhere").unwrap();
        assert!(check_key_destination(&worktree.join("k.p8")).is_err());
        let outside = dir.path().join("keys/AuthKey_X.p8");
        assert!(check_key_destination(&outside).is_ok());
        assert!(
            check_key_destination(Path::new("/mnt/c/Users/a/AuthKey_X.p8"))
                .unwrap_err()
                .contains("Windows drive")
        );
        assert!(check_key_destination(Path::new("relative/k.p8")).is_err());
    }

    #[test]
    fn finds_candidates_in_downloads() {
        let dir = tempfile::tempdir().unwrap();
        let users = dir.path().join("Users");
        for user in ["Ada", "Public"] {
            std::fs::create_dir_all(users.join(user).join("Downloads")).unwrap();
        }
        std::fs::write(users.join("Ada/Downloads/Xcode_27.1.xip"), b"x").unwrap();
        std::fs::write(users.join("Ada/Downloads/AuthKey_2X9R4HXF34.p8"), b"x").unwrap();
        std::fs::write(users.join("Public/Downloads/Xcode_26.xip"), b"x").unwrap();
        let mut paths = Paths::from_home(dir.path().join("home"));
        paths.windows_users = users.clone();
        let downloads = paths.windows_downloads();
        assert_eq!(downloads, vec![users.join("Ada/Downloads")]);
        let xips = find_files(&downloads, is_xcode_xip);
        assert_eq!(xips, vec![users.join("Ada/Downloads/Xcode_27.1.xip")]);
        let keys = find_files(&downloads, is_asc_key);
        assert_eq!(
            key_id_from_file_name(&keys[0]).as_deref(),
            Some("2X9R4HXF34")
        );
    }
}
