//! File permissions for what holds a login or a key: directories `0700`,
//! files `0600`, owned by the user.
//!
//! xtool writes its login token with the default umask (usually `0644`
//! files in `0755` directories), and in API-key mode that token holds the
//! private key. This tool runs xtool with `umask 077` and tightens the
//! modes afterwards.

use std::fs;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::paths::Paths;

pub const DIR_MODE: u32 = 0o700;
pub const FILE_MODE: u32 = 0o600;

/// The user's own uid, from `/proc/self/status` (no `libc` needed).
pub fn current_uid() -> Option<u32> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("Uid:"))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

/// Create `dir` (and its parents) and make it `0700`.
pub fn ensure_private_dir(dir: &Path) -> Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(DIR_MODE)
        .create(dir)
        .map_err(|e| Error::file(dir, &e))?;
    set_mode(dir, DIR_MODE)
}

pub fn set_mode(path: &Path, mode: u32) -> Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|e| Error::file(path, &e))
}

/// Write `bytes` to `path` as a new `0600` file, replacing any file there
/// (written beside it first, then renamed over it).
pub fn write_private_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let Some(parent) = path.parent() else {
        return Err(Error::File {
            path: path.to_path_buf(),
            reason: "no parent directory".into(),
        });
    };
    let temp = parent.join(format!(
        ".{}.tmp",
        path.file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default()
    ));
    let _ = fs::remove_file(&temp);
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(FILE_MODE)
        .open(&temp)
        .map_err(|e| Error::file(&temp, &e))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|e| Error::file(&temp, &e))?;
    drop(file);
    set_mode(&temp, FILE_MODE)?;
    fs::rename(&temp, path).map_err(|e| Error::file(path, &e))
}

/// A path whose mode or owner is looser than it should be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub path: PathBuf,
    pub mode: u32,
    pub wanted: u32,
    pub wrong_owner: bool,
}

impl Finding {
    pub fn describe(&self) -> String {
        if self.wrong_owner {
            format!("{} is not owned by you", self.path.display())
        } else {
            format!(
                "{} is {:04o}, should be {:04o}",
                self.path.display(),
                self.mode,
                self.wanted
            )
        }
    }
}

fn check_one(path: &Path, wanted: u32, uid: Option<u32>) -> Option<Finding> {
    let meta = fs::symlink_metadata(path).ok()?;
    let mode = meta.mode() & 0o7777;
    let wrong_owner = uid.is_some_and(|uid| meta.uid() != uid);
    // Anything for the group or others is too loose.
    (mode & 0o077 != 0 || wrong_owner).then(|| Finding {
        path: path.to_path_buf(),
        mode,
        wanted,
        wrong_owner,
    })
}

/// Every private path that exists: xtool's directory, data directory and
/// token, and this tool's directory, keys and state.
fn private_paths(paths: &Paths) -> Vec<(PathBuf, u32)> {
    let mut list = vec![
        (paths.xtool_config.clone(), DIR_MODE),
        (paths.xtool_data(), DIR_MODE),
        (paths.xtool_token(), FILE_MODE),
        (paths.config.clone(), DIR_MODE),
        (paths.keys_dir(), DIR_MODE),
        (paths.state_file(), FILE_MODE),
    ];
    if let Ok(entries) = fs::read_dir(paths.keys_dir()) {
        for entry in entries.filter_map(|e| e.ok()) {
            list.push((entry.path(), FILE_MODE));
        }
    }
    if let Ok(entries) = fs::read_dir(paths.xtool_data()) {
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            let mode = if path.is_dir() { DIR_MODE } else { FILE_MODE };
            if !list.iter().any(|(p, _)| *p == path) {
                list.push((path, mode));
            }
        }
    }
    list.into_iter().filter(|(p, _)| p.exists()).collect()
}

/// What is looser than it should be.
pub fn check(paths: &Paths) -> Vec<Finding> {
    let uid = current_uid();
    private_paths(paths)
        .into_iter()
        .filter_map(|(path, wanted)| check_one(&path, wanted, uid))
        .collect()
}

/// Tighten every private path to its mode. Ownership cannot be fixed
/// without root, so a path owned by someone else is reported, not changed.
pub fn fix(paths: &Paths) -> Result<Vec<Finding>> {
    let uid = current_uid();
    let mut remaining = Vec::new();
    for (path, wanted) in private_paths(paths) {
        let meta = fs::symlink_metadata(&path).map_err(|e| Error::file(&path, &e))?;
        if meta.file_type().is_symlink() {
            continue;
        }
        if uid.is_some_and(|uid| meta.uid() != uid) {
            remaining.push(Finding {
                path,
                mode: meta.mode() & 0o7777,
                wanted,
                wrong_owner: true,
            });
            continue;
        }
        if meta.mode() & 0o7777 != wanted {
            set_mode(&path, wanted)?;
        }
    }
    Ok(remaining)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().mode() & 0o7777
    }

    #[test]
    fn fixes_loose_xtool_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(dir.path().to_path_buf());
        // As xtool leaves them with a 022 umask.
        fs::create_dir_all(paths.xtool_data()).unwrap();
        fs::set_permissions(&paths.xtool_config, fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(paths.xtool_data(), fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(paths.xtool_token(), b"{}").unwrap();
        fs::set_permissions(paths.xtool_token(), fs::Permissions::from_mode(0o644)).unwrap();

        let findings = check(&paths);
        assert_eq!(findings.len(), 3, "{findings:?}");
        assert!(findings.iter().all(|f| !f.wrong_owner));
        assert!(findings[2].describe().contains("0644"));

        assert!(fix(&paths).unwrap().is_empty());
        assert_eq!(mode(&paths.xtool_config), 0o700);
        assert_eq!(mode(&paths.xtool_data()), 0o700);
        assert_eq!(mode(&paths.xtool_token()), 0o600);
        assert!(check(&paths).is_empty());
    }

    #[test]
    fn private_files_are_created_0600() {
        let dir = tempfile::tempdir().unwrap();
        let keys = dir.path().join("a/keys");
        ensure_private_dir(&keys).unwrap();
        assert_eq!(mode(&keys), 0o700);
        let file = keys.join("k.p8");
        write_private_file(&file, b"one").unwrap();
        write_private_file(&file, b"two").unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"two");
        assert_eq!(mode(&file), 0o600);
        // No temporary file is left behind.
        assert_eq!(fs::read_dir(&keys).unwrap().count(), 1);
    }

    #[test]
    fn missing_paths_are_not_findings() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::from_home(dir.path().to_path_buf());
        assert!(check(&paths).is_empty());
        assert!(fix(&paths).unwrap().is_empty());
    }

    #[test]
    fn uid_is_known() {
        assert!(current_uid().is_some());
    }
}
