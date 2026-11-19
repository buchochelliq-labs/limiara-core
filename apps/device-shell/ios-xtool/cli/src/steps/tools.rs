//! Steps 3 to 6: Swift, xtool, Rust and Node.js, each pinned and verified
//! before it is installed (see `versions.rs`).

use std::path::{Path, PathBuf};

use super::{Ctx, Status, Step};
use crate::checksum;
use crate::cmd::{Cmd, Input};
use crate::download;
use crate::error::{Error, Result};
use crate::parse;
use crate::perms;
use crate::versions::{self, Arch};

// ---------------------------------------------------------------- Swift

pub const SWIFT: Step = Step {
    id: "swift",
    title: "Swift toolchain",
    why: "xtool builds the Swift half of the app with the Swift toolchain for Linux; its guide names \
Swift 6.4. It is installed with swiftly, swift.org's own installer, whose download is checked against \
swift.org's signature first. swiftly checks the toolchain's signature itself.",
    check: check_swift,
    run: run_swift,
    when_done: None,
};

fn check_swift(cx: &mut Ctx) -> Status {
    let Ok(out) = cx.run.capture(&Cmd::new("swift").arg("--version")) else {
        return Status::Missing("swift is not installed".into());
    };
    match parse::swift_version(&out.text()) {
        Some(v) if parse::version_matches(&v, versions::SWIFT_VERSION) => {
            Status::Done(format!("Swift {v}"))
        }
        Some(v) => Status::Problem(format!(
            "Swift {v} is installed; xtool needs {}",
            versions::SWIFT_VERSION
        )),
        None => Status::Missing("swift is not installed".into()),
    }
}

fn run_swift(cx: &mut Ctx) -> Result<()> {
    let arch = cx.host.arch()?;
    let temp = download::temp_dir()?;
    if !cx.run.exists("swiftly") {
        install_swiftly(cx, arch, temp.path())?;
    }
    cx.ui.note(&format!(
        "xtool's Linux guide: {}",
        versions::XTOOL_LINUX_GUIDE
    ));
    let post_install = temp.path().join("post-install.sh");
    cx.ui.note(&format!(
        "Installing Swift {} with swiftly.",
        versions::SWIFT_VERSION
    ));
    cx.run.interactive(
        &Cmd::new("swiftly")
            .args([
                "install",
                versions::SWIFT_VERSION,
                "--use",
                "--assume-yes",
                "--post-install-file",
            ])
            .path_arg(&post_install),
    )?;
    // swiftly writes the system packages the toolchain still needs as an
    // apt-get script; install those packages ourselves rather than run it.
    if let Ok(script) = std::fs::read_to_string(&post_install) {
        let packages = parse::apt_packages_in_script(&script);
        let missing = super::system::missing_packages(cx, &packages);
        super::system::apt_install(cx, &missing, "the packages swiftly says Swift needs")?;
    }
    Ok(())
}

fn install_swiftly(cx: &mut Ctx, arch: Arch, temp: &Path) -> Result<()> {
    let url = versions::swiftly_url(arch);
    let name = format!("swiftly-{}.tar.gz", arch.uname());
    cx.ui
        .note("Downloading swiftly and its signature from download.swift.org.");
    let tarball = download::file(cx.run, &url, temp, &name)?;
    let signature = download::file(cx.run, &format!("{url}.sig"), temp, &format!("{name}.sig"))?;
    let keys = download::file(cx.run, versions::SWIFT_KEYS_URL, temp, "swift-keys.asc")?;
    verify_swift_signature(cx, temp, &keys, &signature, &tarball)?;
    cx.ui
        .done("swiftly's signature is good (swift.org release key).");
    let unpacked = temp.join("swiftly");
    perms::ensure_private_dir(&unpacked)?;
    cx.run.interactive(
        &Cmd::new("tar")
            .arg("-xzf")
            .path_arg(&tarball)
            .arg("-C")
            .path_arg(&unpacked)
            .input(Input::Null),
    )?;
    cx.run
        .interactive(&Cmd::new(unpacked.join("swiftly").to_string_lossy()).args([
            "init",
            "--quiet-shell-followup",
            "--assume-yes",
            "--skip-install",
        ]))
}

fn verify_swift_signature(
    cx: &mut Ctx,
    temp: &Path,
    keys: &Path,
    signature: &Path,
    file: &Path,
) -> Result<()> {
    let home = temp.join("gnupg");
    perms::ensure_private_dir(&home)?;
    let gpg = || {
        Cmd::new("gpg")
            .args(["--batch", "--no-tty", "--homedir"])
            .path_arg(&home)
    };
    let imported = cx.run.capture(&gpg().arg("--import").path_arg(keys))?;
    let what = "swiftly".to_string();
    if !imported.success {
        return Err(Error::SignatureInvalid { what });
    }
    let verified = cx.run.capture(
        &gpg()
            .args(["--status-fd", "1", "--verify"])
            .path_arg(signature)
            .path_arg(file),
    )?;
    let signer = verified
        .success
        .then(|| parse::gpg_valid_signature(&verified.stdout))
        .flatten();
    match signer {
        Some(fpr) if versions::SWIFT_SIGNING_FINGERPRINTS.contains(&fpr.as_str()) => Ok(()),
        _ => {
            let _ = std::fs::remove_file(file);
            Err(Error::SignatureInvalid { what })
        }
    }
}

// ---------------------------------------------------------------- xtool

pub const XTOOL: Step = Step {
    id: "xtool",
    title: "xtool",
    why: "xtool replaces Xcode here: it builds, signs and installs the app. Its AppImage is downloaded \
from xtool's GitHub release (a pinned version) into ~/.local/bin, and checked against the SHA-256 \
recorded for that release before it is made executable.",
    check: check_xtool,
    run: run_xtool,
    when_done: None,
};

pub fn xtool_path(cx: &Ctx) -> PathBuf {
    cx.paths.local_bin().join("xtool")
}

fn check_xtool(cx: &mut Ctx) -> Status {
    let Ok(out) = cx.run.capture(&Cmd::new("xtool").arg("--version")) else {
        return Status::Missing("xtool is not installed".into());
    };
    if !out.success {
        return Status::Missing("xtool is not installed or does not start".into());
    }
    let text = out.text();
    let version = text.split_whitespace().last().unwrap_or("").to_string();
    if version == versions::XTOOL_VERSION {
        Status::Done(format!("xtool {version}"))
    } else {
        Status::Problem(format!(
            "xtool {version} is installed; this tool is tested with {}",
            versions::XTOOL_VERSION
        ))
    }
}

fn run_xtool(cx: &mut Ctx) -> Result<()> {
    let arch = cx.host.arch()?;
    let asset = versions::xtool_asset(arch);
    let mut expected: Vec<String> = Vec::new();
    if let Some(pinned) = arch.xtool_sha256() {
        expected.push(pinned.to_string());
    }
    match download::text(
        cx.run,
        &versions::xtool_release_api(),
        "xtool's release metadata",
    )
    .ok()
    .and_then(|json| parse::github_asset_digest(&json, &asset))
    {
        Some(digest) => expected.push(digest),
        None if expected.is_empty() => {
            return Err(Error::ChecksumUnavailable {
                what: format!("xtool {}", versions::XTOOL_VERSION),
            })
        }
        None => cx
            .ui
            .note("GitHub's record of the checksum could not be read; using the pinned one."),
    }
    let temp = download::temp_dir()?;
    cx.ui.note(&format!(
        "Downloading xtool {} ({asset}).",
        versions::XTOOL_VERSION
    ));
    let file = download::file(cx.run, &versions::xtool_url(arch), temp.path(), &asset)?;
    let refs: Vec<&str> = expected.iter().map(String::as_str).collect();
    let sum = checksum::verify(&file, "the xtool AppImage", &refs)?;
    cx.ui.done(&format!("SHA-256 {sum} matches."));
    let bin = cx.paths.local_bin();
    std::fs::create_dir_all(&bin).map_err(|e| Error::file(&bin, &e))?;
    let target = xtool_path(cx);
    let staged = bin.join(".xtool.new");
    std::fs::copy(&file, &staged).map_err(|e| Error::file(&staged, &e))?;
    perms::set_mode(&staged, 0o755)?;
    std::fs::rename(&staged, &target).map_err(|e| Error::file(&target, &e))?;
    if !path_check(cx).is_done() {
        cx.ui.note(
            "~/.local/bin is not on your PATH yet; Ubuntu adds it when you open a new terminal.",
        );
    }
    Ok(())
}

/// Whether `~/.local/bin` is on the user's own `PATH`.
pub fn path_check(cx: &mut Ctx) -> Status {
    let bin = cx.paths.local_bin();
    let on_path = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d == bin))
        .unwrap_or(false);
    if on_path {
        Status::Done(String::new())
    } else {
        Status::Problem("open a new terminal (Ubuntu's ~/.profile adds it once it exists)".into())
    }
}

// ---------------------------------------------------------------- Rust

pub const RUST: Step = Step {
    id: "rust",
    title: "Rust and the iOS target",
    why: "The app's shell is written in Rust and is built for iPhones (aarch64-apple-ios). rustup \
installs the Rust version the repository pins and the iOS target. If rustup is missing, its installer \
is downloaded (a pinned version) and checked against its published SHA-256 first.",
    check: check_rust,
    run: run_rust,
    when_done: None,
};

fn rust_target_cmd() -> Cmd {
    Cmd::new("rustup").args([
        "target",
        "list",
        "--installed",
        "--toolchain",
        versions::RUST_TOOLCHAIN,
    ])
}

fn check_rust(cx: &mut Ctx) -> Status {
    if !cx.run.exists("rustup") {
        return Status::Missing("rustup is not installed".into());
    }
    match cx.run.capture(&rust_target_cmd()) {
        Ok(out)
            if out.success
                && out
                    .stdout
                    .lines()
                    .any(|l| l.trim() == versions::IOS_RUST_TARGET) =>
        {
            Status::Done(format!(
                "Rust {} with {}",
                versions::RUST_TOOLCHAIN,
                versions::IOS_RUST_TARGET
            ))
        }
        _ => Status::Missing(format!(
            "Rust {} or its {} target is not installed",
            versions::RUST_TOOLCHAIN,
            versions::IOS_RUST_TARGET
        )),
    }
}

fn run_rust(cx: &mut Ctx) -> Result<()> {
    if !cx.run.exists("rustup") {
        let arch = cx.host.arch()?;
        let temp = download::temp_dir()?;
        let url = versions::rustup_url(arch);
        let published = download::text(cx.run, &format!("{url}.sha256"), "rustup-init's checksum")?;
        let published = parse::sha256_file(&published).ok_or(Error::ChecksumUnavailable {
            what: "rustup-init".into(),
        })?;
        let file = download::file(cx.run, &url, temp.path(), "rustup-init")?;
        checksum::verify(&file, "rustup-init", &[arch.rustup_sha256(), &published])?;
        cx.ui.done("rustup-init's SHA-256 matches.");
        perms::set_mode(&file, 0o700)?;
        cx.run.interactive(&Cmd::new(file.to_string_lossy()).args([
            "-y",
            "--profile",
            "minimal",
            "--default-toolchain",
            "none",
        ]))?;
    }
    cx.run.interactive(&Cmd::new("rustup").args([
        "toolchain",
        "install",
        versions::RUST_TOOLCHAIN,
        "--profile",
        "minimal",
    ]))?;
    cx.run.interactive(&Cmd::new("rustup").args([
        "target",
        "add",
        versions::IOS_RUST_TARGET,
        "--toolchain",
        versions::RUST_TOOLCHAIN,
    ]))
}

// ---------------------------------------------------------------- Node

pub const NODE: Step = Step {
    id: "node",
    title: "Node.js and pnpm",
    why: "The app's web shell is built with Node.js 22 and pnpm. Node.js comes from nodejs.org's \
official tarball, checked against its SHA256SUMS and the pinned checksum, into ~/.local; pnpm comes \
through corepack, which ships with Node.js and checks what it downloads.",
    check: check_node,
    run: run_node,
    when_done: None,
};

fn node_ok(cx: &mut Ctx) -> Option<String> {
    let out = cx.run.capture(&Cmd::new("node").arg("--version")).ok()?;
    (out.success && parse::node_major(&out.stdout) == Some(22))
        .then(|| out.stdout.trim().to_string())
}

fn pnpm_ok(cx: &mut Ctx) -> bool {
    cx.run
        .capture(
            &Cmd::new("pnpm")
                .arg("--version")
                .env("COREPACK_ENABLE_DOWNLOAD_PROMPT", "0"),
        )
        .is_ok_and(|o| o.success && o.stdout.trim() == versions::PNPM_VERSION)
}

fn check_node(cx: &mut Ctx) -> Status {
    match (node_ok(cx), pnpm_ok(cx)) {
        (Some(node), true) => {
            Status::Done(format!("Node.js {node}, pnpm {}", versions::PNPM_VERSION))
        }
        (Some(node), false) => Status::Missing(format!(
            "Node.js {node}; pnpm {} missing",
            versions::PNPM_VERSION
        )),
        (None, _) => Status::Missing("Node.js 22 is not installed".into()),
    }
}

fn run_node(cx: &mut Ctx) -> Result<()> {
    if node_ok(cx).is_none() {
        install_node(cx)?;
    }
    let bin = cx.paths.local_bin();
    cx.run.interactive(
        &Cmd::new("corepack")
            .args(["enable", "--install-directory"])
            .path_arg(&bin)
            .arg("pnpm"),
    )?;
    cx.run.interactive(
        &Cmd::new("corepack")
            .args([
                "prepare",
                &format!("pnpm@{}", versions::PNPM_VERSION),
                "--activate",
            ])
            .env("COREPACK_ENABLE_DOWNLOAD_PROMPT", "0"),
    )
}

fn install_node(cx: &mut Ctx) -> Result<()> {
    let arch = cx.host.arch()?;
    let tarball = versions::node_tarball(arch);
    let shasums = download::text(
        cx.run,
        &versions::node_url("SHASUMS256.txt"),
        "Node.js' SHASUMS256.txt",
    )?;
    let listed = parse::shasums_entry(&shasums, &tarball).ok_or(Error::ChecksumUnavailable {
        what: tarball.clone(),
    })?;
    let temp = download::temp_dir()?;
    cx.ui
        .note(&format!("Downloading Node.js {}.", versions::NODE_VERSION));
    let file = download::file(cx.run, &versions::node_url(&tarball), temp.path(), &tarball)?;
    checksum::verify(&file, &tarball, &[arch.node_sha256(), &listed])?;
    cx.ui
        .done("Node.js' SHA-256 matches SHASUMS256.txt and the pinned value.");
    let lib = cx.paths.local.join("lib/nodejs");
    std::fs::create_dir_all(&lib).map_err(|e| Error::file(&lib, &e))?;
    cx.run.interactive(
        &Cmd::new("tar")
            .arg("-xJf")
            .path_arg(&file)
            .arg("-C")
            .path_arg(&lib)
            .input(Input::Null),
    )?;
    let dir = lib.join(tarball.trim_end_matches(".tar.xz"));
    let bin = cx.paths.local_bin();
    std::fs::create_dir_all(&bin).map_err(|e| Error::file(&bin, &e))?;
    for tool in ["node", "npm", "npx", "corepack"] {
        link_tool(&dir.join("bin").join(tool), &bin.join(tool))?;
    }
    Ok(())
}

/// Point `link` at `target`, replacing an older link but never a file.
fn link_tool(target: &Path, link: &Path) -> Result<()> {
    match std::fs::symlink_metadata(link) {
        Ok(meta) if meta.file_type().is_symlink() => {
            std::fs::remove_file(link).map_err(|e| Error::file(link, &e))?;
        }
        Ok(_) => {
            return Err(Error::File {
                path: link.to_path_buf(),
                reason: "a file is already there; move it away and run setup again".into(),
            })
        }
        Err(_) => {}
    }
    std::os::unix::fs::symlink(target, link).map_err(|e| Error::file(link, &e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_replace_links_not_files() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("node");
        std::fs::write(&target, b"").unwrap();
        let link = dir.path().join("bin-node");
        link_tool(&target, &link).unwrap();
        link_tool(&target, &link).unwrap();
        assert_eq!(std::fs::read_link(&link).unwrap(), target);
        let file = dir.path().join("plain");
        std::fs::write(&file, b"keep").unwrap();
        assert!(link_tool(&target, &file).is_err());
        assert_eq!(std::fs::read(&file).unwrap(), b"keep");
    }
}
