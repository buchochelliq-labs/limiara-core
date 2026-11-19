//! Step 11: build and install (`prepare.sh`, then `xtool dev`).

use super::{Ctx, Status, Step};
use crate::cmd::Cmd;
use crate::error::{Error, Result};

pub const BUILD: Step = Step {
    id: "build",
    title: "Build and install the app",
    why: "prepare.sh builds the web shell, the Rust shell for iPhones and the Swift packages (the first \
time takes a while); then xtool builds the app, signs it with your login and installs it on the iPhone.",
    check: check_build,
    run,
    when_done: None,
};

fn check_build(cx: &mut Ctx) -> Status {
    if cx.state.completed.iter().any(|s| s == "build") {
        Status::Done("installed before; `limiara-sideload run` rebuilds it".into())
    } else {
        Status::Missing("not built yet".into())
    }
}

pub fn xtool_dev_cmd(dir: &std::path::Path) -> Cmd {
    Cmd::new("xtool").arg("dev").cwd(dir).private_umask()
}

pub fn run(cx: &mut Ctx) -> Result<()> {
    let Some(repo) = cx.state.repo.clone() else {
        return Err(Error::NotReady(
            "the repository is not set up; run `limiara-sideload setup`".into(),
        ));
    };
    let dir = super::repo::project_dir(&repo);
    if !dir.join("prepare.sh").is_file() {
        return Err(Error::NotReady(format!(
            "{} has no prepare.sh",
            dir.display()
        )));
    }
    cx.ui.note("Running prepare.sh.");
    cx.run.interactive(&Cmd::new("./prepare.sh").cwd(&dir))?;
    cx.ui.explain(
        "Next, in xtool",
        "xtool builds, signs and installs the app. While it runs it may say:\n\
  [bold]Please tap 'trust' on your device...[/]  tap Trust on the iPhone and enter its passcode.\n\
  [bold]Please unlock your device...[/]  unlock the iPhone.\n\
  [bold]The following certificates must be revoked:[/] followed by [bold]Continue? (yes/no)[/]  Apple \
allows only a few development certificates per account. Answering yes revokes the ones listed so xtool \
can make a new one; apps already signed with them (on any device) stop opening until reinstalled. \
Answer no to stop instead.",
    );
    cx.run.interactive(&xtool_dev_cmd(&dir))?;
    super::login::secure_token(cx)?;
    cx.ui.explain(
        "On the iPhone",
        "The first time, open [bold]Settings → General → VPN & Device Management[/], tap your developer \
profile and trust it; the app then opens.\n\
With a free Apple ID the app stops opening after [bold]7 days[/]: run [bold]limiara-sideload run[/] again \
to reinstall it. With a paid membership it lasts a year.",
    );
    cx.state.mark_done("build");
    cx.save_state()
}
