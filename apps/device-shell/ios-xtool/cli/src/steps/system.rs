//! Steps 1 and 2: the machine (WSL, Ubuntu) and the system packages.

use super::{Ctx, Status, Step};
use crate::cmd::{Cmd, Input};
use crate::error::{Error, Result};

pub const WSL: Step = Step {
    id: "wsl",
    title: "WSL with Ubuntu",
    why: "xtool runs on Linux. On Windows that Linux is WSL (Windows Subsystem for Linux) with Ubuntu, \
where everything below is installed. This checks that is where we are.",
    check: check_wsl,
    run: run_wsl,
    when_done: None,
};

fn check_wsl(cx: &mut Ctx) -> Status {
    let os = cx.host.os.clone();
    let name = os
        .as_ref()
        .map(|o| o.pretty_name.clone())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "an unknown Linux".into());
    if !cx.host.wsl {
        return Status::Problem(format!("{name}, not WSL"));
    }
    match os {
        Some(os) if os.is_ubuntu() && os.version().is_some_and(|v| v >= (22, 4)) => {
            Status::Done(format!("{name} on WSL"))
        }
        Some(os) if os.is_ubuntu() => {
            Status::Problem(format!("{name}: Ubuntu 22.04 or newer is needed"))
        }
        _ => Status::Problem(format!("{name}: this tool knows Ubuntu only")),
    }
}

fn run_wsl(cx: &mut Ctx) -> Result<()> {
    cx.ui.warn(
        "Not WSL with Ubuntu",
        "Run this inside WSL's Ubuntu on Windows. To get there, run [bold]setup-windows.ps1[/] from \
apps/device-shell/ios-xtool in PowerShell: it installs WSL with Ubuntu 24.04 (Windows may ask you to \
restart, then run it again), builds this tool inside Ubuntu and starts it. Or by hand: \
[bold]wsl --install -d Ubuntu-24.04[/] in an administrator PowerShell, then open Ubuntu and run \
apps/device-shell/ios-xtool/cli/bootstrap.sh.",
    );
    if !cx.host.wsl {
        return Err(Error::NotWsl(
            "/proc/version does not name Microsoft's WSL kernel".into(),
        ));
    }
    Err(Error::UnsupportedDistro(
        cx.host
            .os
            .as_ref()
            .map(|o| o.pretty_name.clone())
            .unwrap_or_default(),
    ))
}

pub const APT: Step = Step {
    id: "apt",
    title: "System packages",
    why: "The build needs a C toolchain (clang, llvm) for the Rust half, git, curl, rsync, xz and gpg for \
downloads and checks, usbmuxd and libimobiledevice to talk to the iPhone, FUSE for xtool's AppImage, and \
the libraries swift.org lists for its toolchain. They come from Ubuntu's own archive with apt.",
    check: check_apt,
    run: run_apt,
    when_done: None,
};

/// The packages the steps need on this Ubuntu.
pub fn packages(os: Option<&crate::parse::OsRelease>) -> Vec<String> {
    let mut list: Vec<String> = [
        "curl",
        "git",
        "build-essential",
        "clang",
        "llvm",
        "perl",
        "make",
        "rsync",
        "usbmuxd",
        "libimobiledevice-utils",
        "xz-utils",
        "gnupg",
        "ca-certificates",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let default = crate::parse::os_release("ID=ubuntu\nVERSION_ID=\"24.04\"");
    let os = os.unwrap_or(&default);
    list.push(os.fuse_package().to_string());
    list.extend(os.swift_dependencies().iter().map(|s| s.to_string()));
    list.sort();
    list.dedup();
    list
}

pub fn missing_packages(cx: &mut Ctx, wanted: &[String]) -> Vec<String> {
    wanted
        .iter()
        .filter(|pkg| {
            let out = cx.run.capture(
                &Cmd::new("dpkg-query")
                    .args(["-W", "-f", "${Status}"])
                    .arg(pkg.as_str()),
            );
            !out.is_ok_and(|o| o.success && crate::parse::dpkg_installed(&o.stdout))
        })
        .cloned()
        .collect()
}

fn check_apt(cx: &mut Ctx) -> Status {
    if !cx.run.exists("dpkg-query") {
        return Status::Unknown("no dpkg (not Debian or Ubuntu)".into());
    }
    let wanted = packages(cx.host.os.as_ref());
    let missing = missing_packages(cx, &wanted);
    if missing.is_empty() {
        Status::Done(format!("{} packages installed", wanted.len()))
    } else {
        Status::Missing(format!("not installed: {}", missing.join(" ")))
    }
}

pub fn apt_install_cmds(packages: &[String]) -> Vec<Cmd> {
    vec![
        Cmd::new("sudo").args(["apt-get", "update"]),
        Cmd::new("sudo")
            .args(["apt-get", "install", "-y", "--no-install-recommends"])
            .args(packages.iter().cloned()),
    ]
}

/// Install `packages` with apt, after showing the commands and asking.
pub fn apt_install(cx: &mut Ctx, packages: &[String], why: &str) -> Result<()> {
    if packages.is_empty() {
        return Ok(());
    }
    let cmds = apt_install_cmds(packages);
    if !cx.confirm_sudo(&cmds, why)? {
        return Err(Error::Cancelled);
    }
    for cmd in &cmds {
        cx.run.interactive(&cmd.clone().input(Input::Inherit))?;
    }
    Ok(())
}

fn run_apt(cx: &mut Ctx) -> Result<()> {
    let wanted = packages(cx.host.os.as_ref());
    let missing = missing_packages(cx, &wanted);
    apt_install(cx, &missing, "installing packages with apt")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuse_follows_the_release() {
        let noble = crate::parse::os_release("ID=ubuntu\nVERSION_ID=\"24.04\"");
        let jammy = crate::parse::os_release("ID=ubuntu\nVERSION_ID=\"22.04\"");
        assert!(packages(Some(&noble)).contains(&"libfuse2t64".to_string()));
        assert!(packages(Some(&jammy)).contains(&"libfuse2".to_string()));
        for pkg in [
            "usbmuxd",
            "libimobiledevice-utils",
            "clang",
            "llvm",
            "rsync",
        ] {
            assert!(packages(Some(&noble)).contains(&pkg.to_string()));
        }
    }

    #[test]
    fn sudo_only_wraps_apt() {
        for cmd in apt_install_cmds(&["curl".into()]) {
            assert_eq!(cmd.program, "sudo");
            assert_eq!(cmd.args[0], "apt-get");
            assert_eq!(cmd.input, Input::Inherit);
        }
    }
}
