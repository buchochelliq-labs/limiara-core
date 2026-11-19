//! Step 9: logging xtool in to Apple.
//!
//! This tool never reads, keeps, logs or passes on an Apple ID password
//! or a two-factor code. In password mode it asks for the Apple ID only,
//! shows xtool's warnings, then runs `xtool auth login` on the terminal so
//! the password (hidden), the code and the team are typed into xtool. In
//! API-key mode it asks for the Key ID, the Issuer ID and where the key
//! file is; it moves the key into `~/.config/limiara-sideload/keys`
//! (`0600`), and gives xtool those three answers, the key's *path* among
//! them. The key's contents are never printed, logged, put in an argument,
//! an environment variable or the state file.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::{Ctx, Status, Step};
use crate::cmd::{Cmd, Input};
use crate::error::{Error, Result};
use crate::parse::{self, AuthStatus};
use crate::paths;
use crate::perms;
use crate::validate;
use crate::versions;

pub const LOGIN: Step = Step {
    id: "login",
    title: "Apple login for xtool",
    why: "To sign the app and install it on your iPhone, xtool logs in to Apple: either with an App Store \
Connect API key (paid developer membership) or with your Apple ID and password. xtool keeps the login in \
~/.config/xtool; this tool makes that private to you.",
    check: check_login,
    run: run_login,
    when_done: Some(keep_or_switch),
};

/// xtool's login modes, as `AuthMode.description` words them, in its
/// order.
pub const MODE_KEY: &str = "API Key (requires paid Apple Developer Program membership)";
pub const MODE_PASSWORD: &str = "Password (works with any Apple ID but uses private APIs)";
/// xtool's own question (`AuthOperation`).
pub const MODE_QUESTION: &str = "Select login mode";

/// What xtool prints before a password login (`logInWithPassword`).
pub const XTOOL_PASSWORD_WARNING: &str = "Heads up:
  This works by emulating a virtual Mac to interact with Apple services.
  We use a third-party service to create a randomized device ID token
  (Anisette Data) but your credentials are only ever sent to Apple.
  Still, consider using a secondary Apple ID since this is a private API.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Key,
    Password,
}

pub fn auth_status(cx: &mut Ctx) -> Option<AuthStatus> {
    let out = cx
        .run
        .capture(&Cmd::new("xtool").args(["auth", "status"]).private_umask())
        .ok()?;
    out.success.then(|| parse::auth_status(&out.stdout))
}

fn check_login(cx: &mut Ctx) -> Status {
    match auth_status(cx) {
        None => Status::Missing("xtool is needed first".into()),
        Some(AuthStatus::LoggedOut) => Status::Missing("logged out".into()),
        Some(status) => Status::Done(status.summary()),
    }
}

fn keep_or_switch(cx: &mut Ctx) -> Result<()> {
    let options = vec![
        "Keep this login".to_string(),
        "Log in again (switch account or mode)".to_string(),
    ];
    let choice = if cx.state.completed.iter().any(|s| s == "login") {
        0
    } else {
        cx.ui
            .choose("xtool is already logged in. Keep it?", &options, 0)?
    };
    if choice == 1 {
        run_login(cx)?;
    }
    secure_token(cx)
}

pub fn run_login(cx: &mut Ctx) -> Result<()> {
    if let Some(status) = auth_status(cx) {
        if status != AuthStatus::LoggedOut {
            cx.ui.note(&format!(
                "Logged in now: {}. Logging in again replaces it.",
                rich::markup::escape(&status.summary())
            ));
        }
    }
    cx.ui.explain(
        "Choosing a login mode",
        &format!(
            "[bold]API Key:[/] if you have a paid Apple Developer Program membership, this is the \
recommended option. It relies on the public App Store Connect API. Create a [bold]Team Key[/] with the \
[bold]App Manager[/] role ({guide}).\n\n\
[bold]Password:[/] if you aren't enrolled in the paid developer program, use password-based \
authentication. It relies on private Apple APIs to authenticate, so you may want to create a throwaway \
Apple ID to be extra cautious.\n\n\
Either way your credentials are only sent to Apple. This tool never sees a password or a code.",
            guide = versions::ASC_KEYS_GUIDE
        ),
    );
    let options = vec![MODE_KEY.to_string(), MODE_PASSWORD.to_string()];
    let mode = match cx.ui.choose(MODE_QUESTION, &options, 0)? {
        0 => Mode::Key,
        _ => Mode::Password,
    };
    match mode {
        Mode::Key => key_login(cx)?,
        Mode::Password => password_login(cx)?,
    }
    secure_token(cx)?;
    cx.save_state()
}

/// `xtool auth login` in password mode: the Apple ID on the command line
/// (it is not a secret), and nothing else. xtool asks for the password
/// itself, without echo, then the two-factor code and the team.
pub fn password_login_cmd(apple_id: &str) -> Cmd {
    Cmd::new("xtool")
        .args(["auth", "login", "--mode", "password", "-u", apple_id])
        .input(Input::Inherit)
        .private_umask()
}

/// `xtool auth login` in API-key mode, with the three answers it reads:
/// `Key ID: `, `Issuer ID: `, `Key path: `. The key's path, never its
/// contents.
pub fn key_login_cmd(key_id: &str, issuer_id: &str, key_path: &Path) -> Cmd {
    Cmd::new("xtool")
        .args(["auth", "login", "--mode", "key"])
        .input(Input::Lines(vec![
            key_id.to_string(),
            issuer_id.to_string(),
            key_path.to_string_lossy().into_owned(),
        ]))
        .private_umask()
}

fn password_login(cx: &mut Ctx) -> Result<()> {
    cx.ui.say("xtool says:");
    cx.ui.say_verbatim(XTOOL_PASSWORD_WARNING);
    cx.ui.say("");
    let default = cx.state.apple_id.clone();
    let apple_id = cx
        .ui
        .ask("Apple ID", default.as_deref(), Rc::new(validate::apple_id))?;
    cx.ui.explain(
        "Next, in xtool",
        "xtool now asks, in this terminal:\n\
  [bold]Password:[/]  your Apple ID password. Nothing shows as you type.\n\
  [bold]Code:[/]      the two-factor code Apple shows on your other devices or sends by text.\n\
  [bold]Select a team[/] if your Apple ID belongs to more than one.\n\
These go straight to xtool (and from there only to Apple); this tool does not see or keep them.",
    );
    cx.state.apple_id = Some(apple_id.clone());
    cx.state.login_mode = Some("password".into());
    cx.save_state()?;
    cx.run.interactive(&password_login_cmd(&apple_id))
}

fn key_login(cx: &mut Ctx) -> Result<()> {
    cx.ui.explain(
        "API key",
        &format!(
            "In App Store Connect, Users and Access → Integrations → Team Keys, create a key with the \
[bold]App Manager[/] role ({guide}). Note its [bold]Key ID[/] and the [bold]Issuer ID[/] shown above the \
list, and download the key: a file named AuthKey_<KEYID>.p8. Apple lets you download it [bold]only once[/], \
so keep a backup in a password manager.",
            guide = versions::ASC_KEYS_GUIDE
        ),
    );
    let found = key_candidates(cx);
    let guessed = found.iter().find_map(|p| paths::key_id_from_file_name(p));
    let key_id = cx
        .ui
        .ask("Key ID", guessed.as_deref(), Rc::new(validate::key_id))?;
    let issuer_id = cx.ui.ask("Issuer ID", None, Rc::new(validate::issuer_id))?;
    let source = choose_key_file(cx, &key_id, &found)?;
    let stored = store_key(cx, &source, &key_id)?;
    cx.ui.done(&format!(
        "The key is in {} (only you can read it).",
        rich::markup::escape(&stored.to_string_lossy())
    ));
    offer_to_delete_original(cx, &source, &stored)?;
    cx.state.login_mode = Some("key".into());
    cx.state.key_file = Some(stored.clone());
    cx.save_state()?;
    cx.ui
        .note("Giving xtool the Key ID, the Issuer ID and the key's path.");
    cx.run
        .interactive(&key_login_cmd(&key_id, &issuer_id, &stored))
}

fn key_candidates(cx: &Ctx) -> Vec<PathBuf> {
    let mut dirs = cx.paths.windows_downloads();
    dirs.push(cx.paths.home.join("Downloads"));
    dirs.push(cx.paths.keys_dir());
    paths::find_files(&dirs, paths::is_asc_key)
}

fn choose_key_file(cx: &mut Ctx, key_id: &str, found: &[PathBuf]) -> Result<PathBuf> {
    let name = format!("AuthKey_{key_id}.p8");
    let matching: Vec<PathBuf> = found
        .iter()
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy() == name))
        .cloned()
        .collect();
    let mut options: Vec<String> = matching.iter().map(|p| p.display().to_string()).collect();
    options.push("Type or paste the path…".into());
    let question = if matching.is_empty() {
        format!("{name} is not in your Downloads. Where is it?")
    } else {
        "Which key file?".to_string()
    };
    let choice = cx.ui.choose(&question, &options, 0)?;
    if choice < matching.len() {
        return Ok(matching[choice].clone());
    }
    let home = cx.paths.home.clone();
    let typed = cx.ui.ask(
        "Path to the .p8 key file",
        None,
        Rc::new(move |typed: &str| {
            let path = paths::normalize_user_path(typed, &home, None)
                .ok_or("Enter the key file's path.")?;
            if !path.is_file() {
                return Err(format!("There is no file at {}.", path.display()));
            }
            Ok(path.to_string_lossy().into_owned())
        }),
    )?;
    Ok(PathBuf::from(typed))
}

/// Copy the key into the private keys folder, after checking it looks like
/// a PEM private key. Nothing of the key is shown.
pub fn store_key(cx: &mut Ctx, source: &Path, key_id: &str) -> Result<PathBuf> {
    let dir = cx.paths.keys_dir();
    let target = dir.join(format!("AuthKey_{key_id}.p8"));
    paths::check_key_destination(&target).map_err(|reason| Error::UnsafeKeyLocation {
        path: target.clone(),
        reason,
    })?;
    let meta = std::fs::metadata(source).map_err(|e| Error::file(source, &e))?;
    if meta.len() > 64 * 1024 {
        return Err(Error::InvalidKey(
            validate::PemProblem::TooLarge.describe().into(),
        ));
    }
    let mut bytes = std::fs::read(source).map_err(|e| Error::file(source, &e))?;
    let checked = validate::sniff_private_key_pem(&bytes);
    let result = match checked {
        Err(problem) => Err(Error::InvalidKey(problem.describe().into())),
        Ok(()) => perms::ensure_private_dir(&cx.paths.config)
            .and_then(|()| perms::ensure_private_dir(&dir))
            .and_then(|()| {
                if same_file(source, &target) {
                    perms::set_mode(&target, perms::FILE_MODE)
                } else {
                    perms::write_private_file(&target, &bytes)
                }
            }),
    };
    // Overwrite the copy in memory before it is freed.
    bytes.iter_mut().for_each(|b| *b = 0);
    drop(bytes);
    result.map(|()| target)
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn offer_to_delete_original(cx: &mut Ctx, source: &Path, stored: &Path) -> Result<()> {
    if same_file(source, stored) {
        return Ok(());
    }
    let shown = rich::markup::escape(&source.to_string_lossy());
    if paths::is_windows_mount(source) {
        cx.ui.warn(
            "Delete the copy in Windows?",
            &format!(
                "{shown} is on your Windows drive. Files there (NTFS under /mnt) can be read by any Windows \
app, cloud sync and backup, so a private key should not stay there. Apple lets you download the key \
only once: keep your backup in a password manager, not in Downloads."
            ),
        );
    } else if let Some(tree) = paths::git_work_tree(source) {
        cx.ui.warn(
            "Delete the copy in the git tree?",
            &format!(
                "{shown} is inside the git working tree {}, where it could be committed by mistake.",
                rich::markup::escape(&tree.to_string_lossy())
            ),
        );
    } else {
        cx.ui.say(&format!(
            "The original is still at {shown}; the copy in the keys folder is the one xtool uses."
        ));
    }
    if cx.ui.confirm("Delete the original key file now?", true)? {
        std::fs::remove_file(source).map_err(|e| Error::file(source, &e))?;
        cx.ui.done("Deleted.");
    }
    Ok(())
}

/// Make xtool's directory and token private (xtool writes them with the
/// default umask; in API-key mode the token holds the key).
pub fn secure_token(cx: &mut Ctx) -> Result<()> {
    let left = perms::fix(&cx.paths)?;
    if left.is_empty() {
        if cx.paths.xtool_token().exists() {
            cx.ui
                .done("xtool's login token is private (0600, in a 0700 folder).");
        }
        Ok(())
    } else {
        Err(Error::NotReady(
            left.iter()
                .map(perms::Finding::describe)
                .collect::<Vec<_>>()
                .join("; "),
        ))
    }
}

pub fn token_permissions(cx: &mut Ctx) -> Status {
    if !cx.paths.xtool_token().exists() {
        return Status::Missing("no token yet".into());
    }
    let findings = perms::check(&cx.paths);
    if findings.is_empty() {
        Status::Done("private (0700 folders, 0600 files)".into())
    } else {
        Status::Problem(format!(
            "{} (run `limiara-sideload login` or doctor --fix)",
            findings
                .iter()
                .map(perms::Finding::describe)
                .collect::<Vec<_>>()
                .join("; ")
        ))
    }
}

pub fn key_file_check(cx: &mut Ctx) -> Status {
    let Some(path) = cx.state.key_file.clone() else {
        return Status::Unknown("not using an API key".into());
    };
    if !path.exists() {
        return Status::Problem(format!("{} is gone; log in again", path.display()));
    }
    if let Err(reason) = paths::check_key_destination(&path) {
        return Status::Problem(reason);
    }
    match std::fs::metadata(&path) {
        Ok(meta) if std::os::unix::fs::PermissionsExt::mode(&meta.permissions()) & 0o077 == 0 => {
            Status::Done(format!("{} (0600)", path.display()))
        }
        _ => Status::Problem(format!("{} is readable by others", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::fake::FakeRunner;
    use crate::state::State;
    use crate::steps::Host;
    use crate::ui::testing::{Answer, ScriptUi};
    use std::os::unix::fs::MetadataExt;

    const KEY_ID: &str = "2X9R4HXF34";
    const ISSUER: &str = "57246542-96fe-1a63-e053-0824d011072a";

    fn ctx<'a>(ui: &'a mut ScriptUi, run: &'a mut FakeRunner, home: &Path) -> Ctx<'a> {
        let mut paths = paths::Paths::from_home(home.to_path_buf());
        paths.windows_users = home.join("mnt-c-Users");
        Ctx {
            ui,
            run,
            paths,
            state: State::default(),
            host: Host {
                wsl: true,
                os: None,
                arch: None,
            },
        }
    }

    fn logged_out() -> FakeRunner {
        FakeRunner::default().answer("xtool auth status", "Logged out\n")
    }

    #[test]
    fn password_mode_shows_xtools_warning_and_never_handles_the_password() {
        let home = tempfile::tempdir().unwrap();
        let mut ui = ScriptUi::new([
            Answer::Keys("down enter"),
            Answer::Text("me@example.com".into()),
        ]);
        let mut run = logged_out();
        {
            let mut cx = ctx(&mut ui, &mut run, home.path());
            run_login(&mut cx).unwrap();
            assert_eq!(cx.state.apple_id.as_deref(), Some("me@example.com"));
        }
        let transcript = ui.transcript();
        // xtool's question and both of its mode descriptions, verbatim.
        assert!(transcript.contains("? Select login mode"));
        assert!(
            transcript.contains("(•) API Key (requires paid Apple Developer Program membership)")
        );
        assert!(transcript.contains("( ) Password (works with any Apple ID but uses private APIs)"));
        // xtool's warning, verbatim and before the Apple ID question.
        let warning = transcript
            .find(XTOOL_PASSWORD_WARNING)
            .expect("warning shown verbatim");
        let question = transcript.find("? Apple ID").unwrap();
        assert!(warning < question);
        // The install guide's advice.
        assert!(transcript.contains("throwaway"));
        assert_eq!(ui.questions, ["Select login mode", "Apple ID"]);

        // Exactly one interactive command: xtool, on the terminal, with
        // the Apple ID and no password.
        assert_eq!(run.ran.len(), 1);
        let cmd = &run.ran[0];
        assert_eq!(cmd.input, Input::Inherit);
        assert_eq!(
            cmd.argv()[3..],
            [
                "xtool",
                "auth",
                "login",
                "--mode",
                "password",
                "-u",
                "me@example.com"
            ]
        );
        assert!(!cmd.argv().iter().any(|a| a == "-p" || a == "--password"));
        assert!(cmd.private_umask);
    }

    #[test]
    fn key_mode_moves_the_key_and_passes_only_its_path() {
        let home = tempfile::tempdir().unwrap();
        let downloads = home.path().join("mnt-c-Users/Ada/Downloads");
        std::fs::create_dir_all(&downloads).unwrap();
        let pem = crate::validate::tests::fake_pem();
        let original = downloads.join(format!("AuthKey_{KEY_ID}.p8"));
        std::fs::write(&original, &pem).unwrap();

        let mut ui = ScriptUi::new([
            Answer::Keys("enter"),
            Answer::Text(KEY_ID.into()),
            Answer::Text(ISSUER.into()),
            Answer::Keys("enter"),
            Answer::Yes(true),
        ]);
        let mut run = logged_out();
        let stored;
        {
            let mut cx = ctx(&mut ui, &mut run, home.path());
            run_login(&mut cx).unwrap();
            stored = cx.state.key_file.clone().unwrap();
            assert_eq!(
                stored,
                cx.paths.keys_dir().join(format!("AuthKey_{KEY_ID}.p8"))
            );
            // The state file names the key's place, never its contents.
            let state = std::fs::read_to_string(cx.paths.state_file()).unwrap();
            assert!(!state.contains("PRIVATE KEY"));
            let body_line = pem.lines().nth(1).unwrap();
            assert!(!state.contains(body_line));
        }
        assert_eq!(std::fs::read_to_string(&stored).unwrap(), pem);
        assert_eq!(std::fs::metadata(&stored).unwrap().mode() & 0o777, 0o600);
        assert_eq!(
            std::fs::metadata(stored.parent().unwrap()).unwrap().mode() & 0o777,
            0o700
        );
        // The original in Downloads was offered for deletion and deleted.
        assert!(!original.exists());

        let transcript = ui.transcript();
        assert!(transcript.contains("Team Key"));
        assert!(transcript.contains("App Manager"));
        assert!(transcript.contains("only once"));
        assert_eq!(
            ui.questions,
            [
                "Select login mode",
                "Key ID",
                "Issuer ID",
                "Which key file?",
                "Delete the original key file now?"
            ]
        );
        // Nothing of the key was printed.
        for line in pem.lines() {
            assert!(!transcript.contains(line), "key text printed");
        }

        assert_eq!(run.ran.len(), 1);
        let cmd = &run.ran[0];
        assert_eq!(cmd.argv()[3..], ["xtool", "auth", "login", "--mode", "key"]);
        let Input::Lines(lines) = &cmd.input else {
            panic!("xtool's three answers are piped");
        };
        assert_eq!(
            lines,
            &[
                KEY_ID.to_string(),
                ISSUER.to_string(),
                stored.to_string_lossy().into_owned()
            ]
        );
        for part in cmd
            .argv()
            .iter()
            .chain(lines)
            .chain(cmd.env.iter().map(|(_, v)| v))
        {
            for line in pem.lines() {
                assert!(!part.contains(line), "key contents reached the command");
            }
        }
    }

    #[test]
    fn key_mode_rejects_invalid_answers_until_valid() {
        let validate: crate::ui::Validator = Rc::new(validate::key_id);
        let mut ui = ScriptUi::new([Answer::Text("lowercase1".into())]);
        let err = crate::ui::Ui::ask(&mut ui, "Key ID", None, validate).unwrap_err();
        assert_eq!(err.code(), "LS030_INVALID_INPUT");
        assert!(ui.transcript().contains("10 characters"));
    }

    #[test]
    fn a_file_that_is_not_a_key_is_refused_without_quoting_it() {
        let home = tempfile::tempdir().unwrap();
        let source = home.path().join("AuthKey_X.p8");
        std::fs::write(&source, "not a key: secret-looking-text").unwrap();
        let mut ui = ScriptUi::new([]);
        let mut run = FakeRunner::default();
        let mut cx = ctx(&mut ui, &mut run, home.path());
        let err = store_key(&mut cx, &source, KEY_ID).unwrap_err();
        assert_eq!(err.code(), "LS031_INVALID_KEY");
        assert!(!err.to_string().contains("secret-looking-text"));
        assert!(!cx
            .paths
            .keys_dir()
            .join(format!("AuthKey_{KEY_ID}.p8"))
            .exists());
    }

    #[test]
    fn keys_folder_inside_a_git_tree_is_refused() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".git")).unwrap();
        let source = home.path().join("k.p8");
        std::fs::write(&source, crate::validate::tests::fake_pem()).unwrap();
        let mut ui = ScriptUi::new([]);
        let mut run = FakeRunner::default();
        let mut cx = ctx(&mut ui, &mut run, home.path());
        assert_eq!(
            store_key(&mut cx, &source, KEY_ID).unwrap_err().code(),
            "LS032_UNSAFE_KEY_LOCATION"
        );
    }

    #[test]
    fn neither_mode_puts_a_secret_on_argv() {
        let pem = crate::validate::tests::fake_pem();
        let key = key_login_cmd(
            KEY_ID,
            ISSUER,
            Path::new("/home/a/.config/limiara-sideload/keys/k.p8"),
        );
        let password = password_login_cmd("me@example.com");
        for cmd in [&key, &password] {
            let argv = cmd.argv();
            assert!(!argv
                .iter()
                .any(|a| a == "-p" || a.starts_with("--password")));
            assert!(!argv
                .iter()
                .any(|a| a.contains("BEGIN") || pem.lines().any(|l| a.contains(l))));
            assert!(cmd.env.is_empty());
        }
        assert_eq!(password.input, Input::Inherit);
    }

    #[test]
    fn already_logged_in_can_be_kept() {
        let home = tempfile::tempdir().unwrap();
        let mut ui = ScriptUi::new([Answer::Keys("enter")]);
        let mut run = FakeRunner::default().answer(
            "xtool auth status",
            "Logged in.\n- ASC key ID: 2X9R4HXF34\n- Issuer ID: 57246542-96fe-1a63-e053-0824d011072a\n",
        );
        let mut cx = ctx(&mut ui, &mut run, home.path());
        assert_eq!(
            check_login(&mut cx),
            Status::Done("API key 2X9R4HXF34".into())
        );
        keep_or_switch(&mut cx).unwrap();
        assert!(run.ran.is_empty());
    }
}
