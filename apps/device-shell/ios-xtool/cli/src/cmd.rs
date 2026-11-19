//! Commands, built as data so tests can see exactly what would run, and
//! the runner that runs them.
//!
//! No command this tool builds carries a password, a code or a key's
//! contents: those never reach this process (see `steps::login`).

use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::error::{Error, Result};

/// What the child reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Input {
    /// The terminal: the user types into the child (sudo, xtool).
    Inherit,
    /// Nothing (probes).
    Null,
    /// These lines, then end of input. Only for answers that are not
    /// secrets.
    Lines(Vec<String>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cmd {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub input: Input,
    /// Run under `umask 077`, so what the child writes is private.
    pub private_umask: bool,
    pub env: Vec<(String, String)>,
}

impl Cmd {
    pub fn new(program: impl Into<String>) -> Cmd {
        Cmd {
            program: program.into(),
            args: Vec::new(),
            cwd: None,
            input: Input::Inherit,
            private_umask: false,
            env: Vec::new(),
        }
    }

    pub fn arg(mut self, arg: impl Into<String>) -> Cmd {
        self.args.push(arg.into());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Cmd
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn path_arg(self, path: &Path) -> Cmd {
        self.arg(path.to_string_lossy().into_owned())
    }

    pub fn cwd(mut self, dir: &Path) -> Cmd {
        self.cwd = Some(dir.to_path_buf());
        self
    }

    pub fn input(mut self, input: Input) -> Cmd {
        self.input = input;
        self
    }

    pub fn private_umask(mut self) -> Cmd {
        self.private_umask = true;
        self
    }

    pub fn env(mut self, key: &str, value: &str) -> Cmd {
        self.env.push((key.into(), value.into()));
        self
    }

    /// The argv that is executed, the umask wrapper included.
    pub fn argv(&self) -> Vec<String> {
        let mut argv = Vec::new();
        if self.private_umask {
            argv.extend([
                "sh".to_string(),
                "-c".to_string(),
                "umask 077 && exec \"$0\" \"$@\"".to_string(),
            ]);
        }
        argv.push(self.program.clone());
        argv.extend(self.args.iter().cloned());
        argv
    }

    /// The command as a person would type it (without the umask wrapper).
    pub fn display(&self) -> String {
        std::iter::once(&self.program)
            .chain(self.args.iter())
            .map(|a| shell_quote(a))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The program's file name, for messages.
    pub fn name(&self) -> String {
        Path::new(&self.program)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.program.clone())
    }
}

pub fn shell_quote(arg: &str) -> String {
    if !arg.is_empty()
        && arg
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./=:@,+%".contains(&b))
    {
        arg.to_string()
    } else {
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Captured {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

impl Captured {
    #[cfg(test)]
    pub fn ok(stdout: &str) -> Captured {
        Captured {
            success: true,
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }

    /// stdout then stderr: what a tool printed, for parsing.
    pub fn text(&self) -> String {
        format!("{}{}", self.stdout, self.stderr)
    }
}

pub trait Runner {
    /// Run with output captured, for checks. Never interactive.
    fn capture(&mut self, cmd: &Cmd) -> Result<Captured>;
    /// Run on the terminal; an unsuccessful exit is an error.
    fn interactive(&mut self, cmd: &Cmd) -> Result<()>;
    /// Whether `program` is on the `PATH` the children get.
    fn exists(&mut self, program: &str) -> bool;
}

/// The real runner: children get `PATH` with the tool directories first.
pub struct System {
    path: OsString,
}

impl System {
    pub fn new(tool_dirs: &[PathBuf]) -> System {
        let mut dirs: Vec<PathBuf> = tool_dirs.to_vec();
        if let Some(path) = std::env::var_os("PATH") {
            dirs.extend(std::env::split_paths(&path));
        }
        let path = std::env::join_paths(dirs)
            .unwrap_or_else(|_| std::env::var_os("PATH").unwrap_or_default());
        System { path }
    }

    fn command(&self, cmd: &Cmd) -> Command {
        let argv = cmd.argv();
        let mut command = Command::new(&argv[0]);
        command.args(&argv[1..]).env("PATH", &self.path);
        for (key, value) in &cmd.env {
            command.env(key, value);
        }
        if let Some(dir) = &cmd.cwd {
            command.current_dir(dir);
        }
        command
    }

    fn spawn_error(cmd: &Cmd, error: &std::io::Error) -> Error {
        Error::Spawn {
            program: cmd.name(),
            reason: crate::error::io_reason(error),
        }
    }
}

impl Runner for System {
    fn capture(&mut self, cmd: &Cmd) -> Result<Captured> {
        let mut command = self.command(cmd);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let output = command.output().map_err(|e| Self::spawn_error(cmd, &e))?;
        Ok(Captured {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }

    fn interactive(&mut self, cmd: &Cmd) -> Result<()> {
        let mut command = self.command(cmd);
        let status = match &cmd.input {
            Input::Lines(lines) => {
                command.stdin(Stdio::piped());
                let mut child = command.spawn().map_err(|e| Self::spawn_error(cmd, &e))?;
                if let Some(mut stdin) = child.stdin.take() {
                    for line in lines {
                        // A child that stops reading early is not an error
                        // here; its exit status says how it went.
                        if writeln!(stdin, "{line}").is_err() {
                            break;
                        }
                    }
                }
                child.wait().map_err(|e| Self::spawn_error(cmd, &e))?
            }
            Input::Null => {
                command.stdin(Stdio::null());
                command.status().map_err(|e| Self::spawn_error(cmd, &e))?
            }
            Input::Inherit => command.status().map_err(|e| Self::spawn_error(cmd, &e))?,
        };
        if status.success() {
            Ok(())
        } else {
            Err(Error::CommandFailed {
                program: cmd.name(),
                status: status.code().map_or_else(
                    || "stopped by a signal".to_string(),
                    |c| format!("exit code {c}"),
                ),
            })
        }
    }

    fn exists(&mut self, program: &str) -> bool {
        if program.contains('/') {
            return Path::new(program).is_file();
        }
        std::env::split_paths(&self.path).any(|dir| dir.join(program).is_file())
    }
}

#[cfg(test)]
pub mod fake {
    //! A runner that records what would run and answers from a script.

    use super::*;

    #[derive(Default)]
    pub struct FakeRunner {
        /// `(display, stdout)` answers for captures, matched by prefix of
        /// the displayed command.
        pub answers: Vec<(String, Captured)>,
        pub programs: Vec<String>,
        pub ran: Vec<Cmd>,
        pub captured: Vec<Cmd>,
    }

    impl FakeRunner {
        pub fn answer(mut self, prefix: &str, stdout: &str) -> FakeRunner {
            self.answers.push((prefix.into(), Captured::ok(stdout)));
            self
        }
    }

    impl Runner for FakeRunner {
        fn capture(&mut self, cmd: &Cmd) -> Result<Captured> {
            self.captured.push(cmd.clone());
            let shown = cmd.display();
            Ok(self
                .answers
                .iter()
                .find(|(prefix, _)| shown.starts_with(prefix.as_str()))
                .map(|(_, c)| c.clone())
                .unwrap_or(Captured {
                    success: false,
                    stdout: String::new(),
                    stderr: "not scripted".into(),
                }))
        }

        fn interactive(&mut self, cmd: &Cmd) -> Result<()> {
            self.ran.push(cmd.clone());
            Ok(())
        }

        fn exists(&mut self, program: &str) -> bool {
            self.programs.iter().any(|p| p == program)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn umask_wrapper_keeps_arguments_separate() {
        let cmd = Cmd::new("xtool").args(["auth", "status"]).private_umask();
        assert_eq!(
            cmd.argv(),
            [
                "sh",
                "-c",
                "umask 077 && exec \"$0\" \"$@\"",
                "xtool",
                "auth",
                "status"
            ]
        );
        assert_eq!(cmd.display(), "xtool auth status");
    }

    #[test]
    fn quoting() {
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(shell_quote("/x/y.p8"), "/x/y.p8");
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn system_runner_runs_and_reports_failure() {
        let mut runner = System::new(&[]);
        let out = runner
            .capture(&Cmd::new("sh").args(["-c", "echo hi"]))
            .unwrap();
        assert!(out.success);
        assert_eq!(out.stdout, "hi\n");
        let err = runner
            .interactive(&Cmd::new("sh").args(["-c", "exit 3"]).input(Input::Null))
            .unwrap_err();
        assert_eq!(err.code(), "LS011_COMMAND_FAILED");
        assert!(err.to_string().contains("exit code 3"));
        assert!(runner.exists("sh"));
        assert!(!runner.exists("definitely-not-a-program-here"));
    }

    #[test]
    fn umask_applies_to_what_the_child_writes() {
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f");
        let mut runner = System::new(&[]);
        runner
            .interactive(
                &Cmd::new("touch")
                    .path_arg(&file)
                    .input(Input::Null)
                    .private_umask(),
            )
            .unwrap();
        assert_eq!(std::fs::metadata(&file).unwrap().mode() & 0o777, 0o600);
    }

    #[test]
    fn lines_are_fed_to_stdin() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out");
        let script = format!("cat > {}", out.display());
        let mut runner = System::new(&[]);
        runner
            .interactive(
                &Cmd::new("sh")
                    .args(["-c", script.as_str()])
                    .input(Input::Lines(vec!["a".into(), "b".into()])),
            )
            .unwrap();
        assert_eq!(std::fs::read_to_string(out).unwrap(), "a\nb\n");
    }
}
