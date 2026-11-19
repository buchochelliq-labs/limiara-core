//! Step 8: Xcode.xip and the Darwin SDK xtool makes from it.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::{Ctx, Status, Step};
use crate::cmd::{Cmd, Input};
use crate::error::{Error, Result};
use crate::parse::{self, SdkFlavor, SdkStatus};
use crate::paths;
use crate::versions;

pub const SDK: Step = Step {
    id: "sdk",
    title: "iOS SDK from Xcode.xip",
    why: "Apple's iOS SDK comes only inside Xcode. xtool takes the Xcode.xip you download from Apple \
and extracts the SDK it needs (into ~/.swiftpm/swift-sdks). Apple requires you to download Xcode \
yourself, signed in with your Apple ID, in a browser.",
    check: check_sdk,
    run: run_sdk,
    when_done: None,
};

/// xtool's own messages (`SDKCommand.swift`, `EnsureSDKOperation`).
pub const XTOOL_SLIM_OUT_OF_DATE: &str =
    "Darwin SDK is out of date, and was installed in 'slim' mode.

Slim SDKs take less disk space, but can't be auto-updated.
Please install a new SDK with
    xtool sdk install [--slim]";
pub const XTOOL_OUT_OF_DATE: &str = "Darwin SDK is out of date. Rebuilding...";
pub const XTOOL_LEGACY: &str = "Darwin SDK is incompatible: built with an older version of xtool.

Requesting re-install. This is a one-time rebuild; after this,
xtool will be able to resolve incompatibilities automatically.";
pub const XTOOL_NOT_INSTALLED: &str = "Now generating the Darwin SDK.";
pub const XTOOL_DOWNLOAD_PROMPT: &str =
    "Please download Xcode from http://developer.apple.com/download/all/?q=Xcode
and enter the path to the downloaded Xcode.xip.";
pub const XTOOL_PATH_PROMPT: &str = "Path to Xcode.xip";

/// A Xcode.xip smaller than this is an incomplete download.
const MIN_XIP_BYTES: u64 = 1 << 30;

pub fn sdk_status(cx: &mut Ctx) -> Option<SdkStatus> {
    let out = cx
        .run
        .capture(&Cmd::new("xtool").args(["sdk", "status"]))
        .ok()?;
    out.success.then(|| parse::sdk_status(&out.stdout))
}

fn check_sdk(cx: &mut Ctx) -> Status {
    match sdk_status(cx) {
        None => Status::Missing("xtool is needed first".into()),
        Some(SdkStatus::NotInstalled) => Status::Missing("not installed".into()),
        Some(status) if status.is_up_to_date() => Status::Done("installed and up to date".into()),
        Some(SdkStatus::Installed { flavor, .. }) => Status::Problem(match flavor {
            SdkFlavor::Slim => "out of date (slim: needs Xcode.xip again)".into(),
            SdkFlavor::Normal => "out of date (xtool can rebuild it)".into(),
            _ => "built by an older xtool (needs Xcode.xip again)".into(),
        }),
    }
}

fn run_sdk(cx: &mut Ctx) -> Result<()> {
    match sdk_status(cx) {
        Some(SdkStatus::Installed {
            flavor: SdkFlavor::Normal,
            ..
        }) => {
            cx.ui.say_verbatim(XTOOL_OUT_OF_DATE);
            return cx
                .run
                .interactive(&Cmd::new("xtool").args(["sdk", "update"]));
        }
        Some(SdkStatus::Installed {
            flavor: SdkFlavor::Slim,
            ..
        }) => cx.ui.say_verbatim(XTOOL_SLIM_OUT_OF_DATE),
        Some(SdkStatus::Installed { .. }) => cx.ui.say_verbatim(XTOOL_LEGACY),
        _ => cx.ui.say_verbatim(XTOOL_NOT_INSTALLED),
    }
    install_from_xip(cx)
}

pub fn install_from_xip(cx: &mut Ctx) -> Result<()> {
    cx.ui.explain(
        &format!("Download Xcode {}", versions::XCODE_VERSION),
        &format!(
            "1. Open [link={url}]{url}[/link] in your Windows browser.\n\
2. Sign in with your Apple ID and accept Apple's licence agreement (Apple requires it; it cannot be \
downloaded with curl).\n\
3. Download [bold]Xcode {version}[/] (the version xtool's guide names). Xcode.xip is a single file of \
7 GB or more, so this takes a while; keep it in Downloads.\n\
4. Extracting it needs room: keep about 40 GB free in WSL. xtool prints its progress as it goes.",
            url = versions::XCODE_DOWNLOAD_URL,
            version = versions::XCODE_VERSION,
        ),
    );
    cx.ui.say_verbatim(XTOOL_DOWNLOAD_PROMPT);
    if cx
        .ui
        .confirm("Open the download page in your Windows browser?", true)?
    {
        open_in_windows_browser(cx, versions::XCODE_DOWNLOAD_URL);
    }
    let xip = choose_xip(cx)?;
    let xip = maybe_copy_into_linux(cx, &xip)?;
    cx.ui
        .note("xtool now extracts the SDK; this can take 10 to 30 minutes.");
    cx.run
        .interactive(&Cmd::new("xtool").args(["sdk", "install"]).path_arg(&xip))
}

pub fn open_in_windows_browser(cx: &mut Ctx, url: &str) {
    // WSL's interop runs Windows programs; explorer.exe opens a URL in
    // the default browser (and exits 1 even when it worked).
    let opened =
        cx.run.exists("explorer.exe") && cx.run.capture(&Cmd::new("explorer.exe").arg(url)).is_ok();
    if !opened {
        cx.ui
            .say(&format!("Open this page in your browser: [bold]{url}[/]"));
    }
}

fn xip_candidates(cx: &Ctx) -> Vec<PathBuf> {
    let mut dirs = cx.paths.windows_downloads();
    dirs.push(cx.paths.home.join("Downloads"));
    dirs.push(cx.paths.home.clone());
    paths::find_files(&dirs, paths::is_xcode_xip)
}

fn size_text(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1e9)
}

fn choose_xip(cx: &mut Ctx) -> Result<PathBuf> {
    loop {
        let found = xip_candidates(cx);
        let mut options: Vec<String> = found
            .iter()
            .map(|p| {
                let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
                format!("{} ({})", p.display(), size_text(size))
            })
            .collect();
        options.push("Type or paste the path…".into());
        options.push("Look again (the download is still running)".into());
        let question = if found.is_empty() {
            "No Xcode*.xip found in your Downloads yet. What now?"
        } else {
            "Which Xcode.xip?"
        };
        let choice = cx.ui.choose(question, &options, 0)?;
        let path = if choice < found.len() {
            found[choice].clone()
        } else if choice == found.len() {
            let home = cx.paths.home.clone();
            let wsl = cx.run.exists("wslpath");
            let typed = cx.ui.ask(
                XTOOL_PATH_PROMPT,
                None,
                Rc::new(move |typed: &str| {
                    let convert = |p: &str| {
                        std::process::Command::new("wslpath")
                            .args(["-u", p])
                            .output()
                            .ok()
                            .filter(|o| o.status.success())
                            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                    };
                    let converter: Option<paths::PathConverter<'_>> =
                        if wsl { Some(&convert) } else { None };
                    let path = paths::normalize_user_path(typed, &home, converter)
                        .ok_or("Enter the path to Xcode.xip.")?;
                    if !path.is_file() {
                        return Err(format!("There is no file at {}.", path.display()));
                    }
                    Ok(path.to_string_lossy().into_owned())
                }),
            )?;
            PathBuf::from(typed)
        } else {
            continue;
        };
        if check_xip(cx, &path)? {
            return Ok(path);
        }
    }
}

/// Whether `path` is a plausible, complete Xcode.xip.
fn check_xip(cx: &mut Ctx, path: &Path) -> Result<bool> {
    let size = std::fs::metadata(path)
        .map_err(|e| Error::file(path, &e))?
        .len();
    if size < MIN_XIP_BYTES {
        cx.ui.warn(
            "This file is too small",
            &format!(
                "{} is only {}; a complete Xcode.xip is 7 GB or more. Is the download finished?",
                rich::markup::escape(&path.to_string_lossy()),
                size_text(size)
            ),
        );
        return Ok(false);
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !name.contains(versions::XCODE_VERSION) {
        cx.ui.warn(
            "Another Xcode version?",
            &format!(
                "xtool's guide names Xcode {}; the file is {}. Another version may not work.",
                versions::XCODE_VERSION,
                rich::markup::escape(&name)
            ),
        );
        return cx.ui.confirm("Use it anyway?", false);
    }
    Ok(true)
}

/// A Xcode.xip on a Windows drive extracts slowly through /mnt; offer to
/// copy it into the Linux file system first.
fn maybe_copy_into_linux(cx: &mut Ctx, xip: &Path) -> Result<PathBuf> {
    if !paths::is_windows_mount(xip) {
        return Ok(xip.to_path_buf());
    }
    cx.ui.explain(
        "Copy it into Linux first?",
        "The file is on a Windows drive. WSL reads /mnt through a slow bridge, so extracting from there \
can take much longer. Copying it into your Linux home first is usually faster overall, but needs as much \
free space again as the file's size.",
    );
    if !cx
        .ui
        .confirm("Copy Xcode.xip into ~/Downloads first?", true)?
    {
        return Ok(xip.to_path_buf());
    }
    let dir = cx.paths.home.join("Downloads");
    std::fs::create_dir_all(&dir).map_err(|e| Error::file(&dir, &e))?;
    let target = dir.join(xip.file_name().unwrap_or_default());
    cx.run.interactive(
        &Cmd::new("rsync")
            .args(["--info=progress2", "--"])
            .path_arg(xip)
            .path_arg(&target)
            .input(Input::Null),
    )?;
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xtool_messages_match_its_source() {
        // As printed by EnsureSDKOperation in xtool's SDKCommand.swift.
        assert!(XTOOL_SLIM_OUT_OF_DATE
            .starts_with("Darwin SDK is out of date, and was installed in 'slim' mode."));
        assert!(XTOOL_DOWNLOAD_PROMPT.ends_with("and enter the path to the downloaded Xcode.xip."));
    }
}
