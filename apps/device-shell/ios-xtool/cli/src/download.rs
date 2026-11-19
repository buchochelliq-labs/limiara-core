//! Downloads, with curl over HTTPS only, into a private temporary
//! directory. Nothing downloaded runs before it is verified.

use std::path::{Path, PathBuf};

use crate::cmd::{Cmd, Input, Runner};
use crate::error::{Error, Result};

/// The curl options every download uses: HTTPS only (also after
/// redirects), TLS 1.2 or newer, fail on HTTP errors.
fn curl() -> Cmd {
    Cmd::new("curl").args([
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "--tlsv1.2",
        "--fail",
        "--location",
        "--retry",
        "3",
    ])
}

pub fn download_cmd(url: &str, to: &Path) -> Cmd {
    curl()
        .args(["--progress-bar", "--compressed", "--output"])
        .path_arg(to)
        .arg(url)
        .input(Input::Null)
}

pub fn fetch_cmd(url: &str) -> Cmd {
    curl()
        .args(["--silent", "--show-error", "--compressed"])
        .arg(url)
}

/// Download `url` to `dir/name`, showing curl's progress bar.
pub fn file(run: &mut dyn Runner, url: &str, dir: &Path, name: &str) -> Result<PathBuf> {
    if !url.starts_with("https://") {
        return Err(Error::Download { what: name.into() });
    }
    let to = dir.join(name);
    run.interactive(&download_cmd(url, &to))
        .map_err(|_| Error::Download { what: name.into() })?;
    Ok(to)
}

/// Fetch a small text resource (a checksum list, release metadata).
pub fn text(run: &mut dyn Runner, url: &str, what: &str) -> Result<String> {
    if !url.starts_with("https://") {
        return Err(Error::Download { what: what.into() });
    }
    let out = run.capture(&fetch_cmd(url))?;
    if out.success {
        Ok(out.stdout)
    } else {
        Err(Error::Download { what: what.into() })
    }
}

/// A private (`0700`) temporary directory, removed when dropped.
pub fn temp_dir() -> Result<tempfile::TempDir> {
    let dir = tempfile::Builder::new()
        .prefix("limiara-sideload-")
        .tempdir()
        .map_err(|e| Error::file(std::env::temp_dir(), &e))?;
    crate::perms::set_mode(dir.path(), crate::perms::DIR_MODE)?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_only() {
        let argv = download_cmd("https://nodejs.org/x", Path::new("/tmp/x")).argv();
        let joined = argv.join(" ");
        assert!(joined.contains("--proto =https"));
        assert!(joined.contains("--proto-redir =https"));
        let mut run = crate::cmd::fake::FakeRunner::default();
        assert!(file(&mut run, "http://example.com/x", Path::new("/tmp"), "x").is_err());
        assert!(run.ran.is_empty());
    }

    #[test]
    fn temp_dirs_are_private() {
        use std::os::unix::fs::MetadataExt;
        let dir = temp_dir().unwrap();
        assert_eq!(std::fs::metadata(dir.path()).unwrap().mode() & 0o777, 0o700);
    }
}
