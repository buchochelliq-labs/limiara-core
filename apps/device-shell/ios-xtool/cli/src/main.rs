//! `limiara-sideload`: a guided setup that takes a Windows machine from
//! nothing to the Limiara iOS shell sideloaded on an iPhone with xtool,
//! inside WSL. See ../README.md.

#![forbid(unsafe_code)]

mod checksum;
mod cmd;
mod download;
mod error;
mod parse;
mod paths;
mod perms;
mod state;
mod steps;
mod ui;
mod validate;
mod versions;

use std::process::ExitCode;

use clap::{Parser, Subcommand};

use crate::error::{Error, Result};
use crate::steps::{Ctx, Host};
use crate::ui::{TermUi, Ui};

#[derive(Parser)]
#[command(
    name = "limiara-sideload",
    version,
    about = "Set up WSL to build the Limiara iOS app and sideload it on an iPhone with xtool",
    long_about = "Set up WSL to build the Limiara iOS app and sideload it on an iPhone with xtool.\n\n\
Passwords and two-factor codes are never asked for by this tool: they are typed straight into \
xtool or sudo. See apps/device-shell/ios-xtool/README.md, \"Security\"."
)]
struct Cli {
    /// Answer yes/no confirmations with their default (never used for
    /// passwords, codes or keys, which are always typed by you).
    #[arg(long, short = 'y', global = true)]
    yes: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// The whole guided setup. Safe to run again: finished steps are skipped.
    Setup,
    /// Check every step and show what is done, missing or wrong.
    Doctor {
        /// Also tighten the permissions of xtool's login token and the key.
        #[arg(long)]
        fix: bool,
    },
    /// Log xtool in to Apple (also to switch account or login mode).
    Login,
    /// Install the iOS SDK from Xcode.xip.
    Sdk,
    /// Connect the iPhone to WSL.
    Phone,
    /// Build the app (prepare.sh) and install it (xtool dev).
    Run,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let mut ui = TermUi::new(cli.yes);
    match run(&cli, &mut ui) {
        Ok(()) => ExitCode::SUCCESS,
        Err(Error::Cancelled) => {
            ui.say("[yellow]Stopped.[/] Run the same command again to carry on.");
            ExitCode::from(130)
        }
        Err(error) => {
            ui.say(&format!(
                "[bold red]error[/] [dim]{}[/]: {}",
                error.code(),
                rich::markup::escape(&error.to_string())
            ));
            ExitCode::FAILURE
        }
    }
}

fn run(cli: &Cli, ui: &mut dyn Ui) -> Result<()> {
    let paths = paths::Paths::detect().ok_or(Error::NotReady("HOME is not set".into()))?;
    let state = state::State::load(&paths.state_file())?;
    let mut runner = cmd::System::new(&paths.tool_dirs());
    let mut cx = Ctx {
        ui,
        run: &mut runner,
        paths,
        state,
        host: Host::detect(),
    };
    match cli.command {
        Command::Setup => steps::setup(&mut cx),
        Command::Doctor { fix } => {
            if fix {
                let left = perms::fix(&cx.paths)?;
                for finding in left {
                    cx.ui.say(&format!(
                        "[red]✗[/] {}",
                        rich::markup::escape(&finding.describe())
                    ));
                }
            }
            steps::doctor(&mut cx).map(|_| ())
        }
        Command::Login => {
            cx.ui.heading("Apple login for xtool");
            steps::login::run_login(&mut cx)
        }
        Command::Sdk => steps::run_step(&mut cx, &steps::sdk::SDK, 1, 1),
        Command::Phone => steps::run_step(&mut cx, &steps::phone::PHONE, 1, 1),
        Command::Run => {
            cx.ui.heading("Build and install");
            steps::build::run(&mut cx)
        }
    }
}
