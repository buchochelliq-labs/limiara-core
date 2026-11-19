//! Step 7: the repository, cloned (or reused) and on the branch that has
//! the xtool project.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::{Ctx, Status, Step};
use crate::cmd::{Cmd, Input};
use crate::error::{Error, Result};
use crate::paths;
use crate::versions;

pub const REPO: Step = Step {
    id: "repo",
    title: "The Limiara repository",
    why: "The app is built from the source: the repository is cloned into your Linux home (not under \
/mnt, where Windows' file system makes builds slow and breaks file permissions), on the branch that \
has apps/device-shell/ios-xtool.",
    check: check_repo,
    run: run_repo,
    when_done: None,
};

pub fn project_dir(repo: &Path) -> PathBuf {
    repo.join(versions::XTOOL_PROJECT_DIR)
}

fn has_project(repo: &Path) -> bool {
    project_dir(repo).join("prepare.sh").is_file()
}

/// The repository this binary was built in, if it runs from a checkout.
pub fn repo_of_this_binary() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors()
        .find(|dir| has_project(dir) && dir.join(".git").exists())
        .map(Path::to_path_buf)
}

fn current_branch(cx: &mut Ctx, repo: &Path) -> Option<String> {
    let out = cx
        .run
        .capture(
            &Cmd::new("git")
                .arg("-C")
                .path_arg(repo)
                .args(["branch", "--show-current"]),
        )
        .ok()?;
    out.success.then(|| out.stdout.trim().to_string())
}

fn check_repo(cx: &mut Ctx) -> Status {
    let Some(repo) = cx.state.repo.clone() else {
        return Status::Missing("not cloned yet".into());
    };
    if !has_project(&repo) {
        return Status::Missing(format!(
            "{} has no {}",
            repo.display(),
            versions::XTOOL_PROJECT_DIR
        ));
    }
    let branch = current_branch(cx, &repo).unwrap_or_default();
    Status::Done(format!("{} ({branch})", repo.display()))
}

fn run_repo(cx: &mut Ctx) -> Result<()> {
    let default = cx
        .state
        .repo
        .clone()
        .or_else(repo_of_this_binary)
        .unwrap_or_else(|| cx.paths.home.join("limiara-core"));
    let home = cx.paths.home.clone();
    let validate: crate::ui::Validator = Rc::new(move |typed: &str| {
        let path = paths::normalize_user_path(typed, &home, None).ok_or("Enter a folder.")?;
        if !path.is_absolute() {
            return Err("Enter a full path, like ~/limiara-core.".into());
        }
        if paths::is_windows_mount(&path) {
            return Err(
                "Use a folder in your Linux home, not under /mnt (a Windows drive).".into(),
            );
        }
        Ok(path.to_string_lossy().into_owned())
    });
    let repo = PathBuf::from(cx.ui.ask(
        "Where should the repository be?",
        Some(&default.to_string_lossy()),
        validate,
    )?);
    let options = vec![
        format!("{} (the xtool experiment)", versions::DEFAULT_BRANCH),
        format!("{} (once the experiment is merged)", versions::MAIN_BRANCH),
        "Another branch…".to_string(),
    ];
    let branch = match cx.ui.choose("Which branch?", &options, 0)? {
        0 => versions::DEFAULT_BRANCH.to_string(),
        1 => versions::MAIN_BRANCH.to_string(),
        _ => cx
            .ui
            .ask("Branch name", None, Rc::new(crate::validate::branch))?,
    };
    if repo.join(".git").exists() {
        reuse(cx, &repo, &branch)?;
    } else if repo.exists()
        && std::fs::read_dir(&repo)
            .map_err(|e| Error::file(&repo, &e))?
            .next()
            .is_some()
    {
        return Err(Error::NotReady(format!(
            "{} exists and is not a git repository; choose another folder",
            repo.display()
        )));
    } else {
        cx.run.interactive(
            &Cmd::new("git")
                .args(["clone", "--branch", &branch, versions::REPO_URL])
                .path_arg(&repo),
        )?;
    }
    if !has_project(&repo) {
        return Err(Error::NotReady(format!(
            "branch {branch} has no {}; choose the branch that has it",
            versions::XTOOL_PROJECT_DIR
        )));
    }
    cx.state.repo = Some(repo);
    cx.state.branch = Some(branch);
    cx.save_state()
}

fn reuse(cx: &mut Ctx, repo: &Path, branch: &str) -> Result<()> {
    let git = || Cmd::new("git").arg("-C").path_arg(repo);
    let origin = cx
        .run
        .capture(&git().args(["remote", "get-url", "origin"]))?;
    if !origin.stdout.contains("buchochelliq-labs/limiara-core") {
        cx.ui.warn(
            "Another repository",
            &format!(
                "{} is a git repository whose origin is not {}.",
                rich::markup::escape(&repo.to_string_lossy()),
                versions::REPO_URL
            ),
        );
        if !cx.ui.confirm("Use it anyway?", false)? {
            return Err(Error::Cancelled);
        }
    }
    if current_branch(cx, repo).as_deref() == Some(branch) && has_project(repo) {
        cx.ui.done(&format!(
            "Reusing {} on {branch}.",
            rich::markup::escape(&repo.to_string_lossy())
        ));
        return Ok(());
    }
    let status = cx.run.capture(&git().args(["status", "--porcelain"]))?;
    if !status.stdout.trim().is_empty() {
        return Err(Error::NotReady(format!(
            "{} has uncommitted changes; commit or stash them, then run setup again",
            repo.display()
        )));
    }
    cx.run.interactive(
        &git()
            .args(["fetch", "origin"])
            .arg(format!("+refs/heads/{branch}:refs/remotes/origin/{branch}"))
            .input(Input::Null),
    )?;
    cx.run
        .interactive(&git().args(["switch", branch]).input(Input::Null))
}
