//! The assistant harness's command line.
//!
//! ```text
//! assistant-harness run     [--assistant siri|gemini|both] [PATH...]
//! assistant-harness export  --out DIR [--assistant siri|gemini|both] [PATH...]
//! assistant-harness session [--assistant siri|gemini]
//! assistant-harness schema
//! ```
//!
//! - `run` runs the scripts (the conformance suite under
//!   `tests/assistant-harness/` when no path is given, with its coverage
//!   check) and exits non-zero on any failure.
//! - `export` runs them and writes the contract file, the scripts and one
//!   transcript per script and assistant, for native plugins' own tests.
//! - `session` sets up the reference world and reads steps, one JSON
//!   object a line, on standard input, answering each with
//!   `{"ok": true, "document": ...}` or `{"ok": false, "error": "..."}`.
//!   A native plugin's tests can route their bridge binding through it.
//! - `schema` prints the contract file.

#![forbid(unsafe_code)]

use assistant_harness::coverage::Coverage;
use assistant_harness::runner::Session;
use assistant_harness::schema::Contract;
use assistant_harness::script::{self, Step};
use platform_objects::Assistant;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

struct Options {
    command: String,
    assistants: Vec<Assistant>,
    paths: Vec<PathBuf>,
    out: Option<PathBuf>,
}

const USAGE: &str = "usage: assistant-harness run|export|session|schema \
    [--assistant siri|gemini|both] [--out DIR] [PATH...]";

fn options() -> Result<Options, String> {
    let mut arguments = std::env::args().skip(1);
    let command = arguments.next().ok_or(USAGE)?;
    let mut options = Options {
        command,
        assistants: vec![Assistant::Siri],
        paths: Vec::new(),
        out: None,
    };
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--assistant" => {
                options.assistants = match arguments.next().as_deref() {
                    Some("siri") => vec![Assistant::Siri],
                    Some("gemini") => vec![Assistant::Gemini],
                    Some("both") => vec![Assistant::Siri, Assistant::Gemini],
                    _ => return Err(USAGE.into()),
                }
            }
            "--out" => options.out = Some(arguments.next().ok_or(USAGE)?.into()),
            "--help" | "-h" => return Err(USAGE.into()),
            path => options.paths.push(path.into()),
        }
    }
    Ok(options)
}

#[tokio::main]
async fn main() -> ExitCode {
    let options = match options() {
        Ok(options) => options,
        Err(usage) => {
            eprintln!("{usage}");
            return ExitCode::from(2);
        }
    };
    let outcome = match options.command.as_str() {
        "run" | "export" => run(&options).await,
        "session" => session(&options).await,
        "schema" => {
            print!("{}", platform_device::assistant::SCHEMA);
            Ok(true)
        }
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match outcome {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("assistant-harness: {error:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(options: &Options) -> anyhow::Result<bool> {
    let whole_suite = options.paths.is_empty();
    let report = assistant_harness::run(&options.paths, &options.assistants, whole_suite).await?;
    for passed in &report.passed {
        println!("ok      {passed}");
    }
    for (name, failure) in &report.failed {
        println!("FAILED  {name}\n  {failure}");
    }
    for missing in &report.missing {
        println!("NOT COVERED  {missing}");
    }
    println!(
        "{} passed, {} failed{}",
        report.passed.len(),
        report.failed.len(),
        if whole_suite {
            format!("; coverage: {}", report.summary)
        } else {
            String::new()
        }
    );
    if options.command == "export" {
        let out = options
            .out
            .clone()
            .ok_or_else(|| anyhow::anyhow!("export needs --out DIR"))?;
        let mut scripts = Vec::new();
        if whole_suite {
            scripts = script::discover(&script::suite_dir())?;
        }
        for path in &options.paths {
            scripts.extend(script::discover(path)?);
        }
        assistant_harness::export(&report, &scripts, &out)?;
        println!("exported to {}", out.display());
    }
    Ok(report.ok())
}

async fn session(options: &Options) -> anyhow::Result<bool> {
    let contract = Contract::bundled()?;
    let mut coverage = Coverage::default();
    let platform = options
        .assistants
        .first()
        .copied()
        .unwrap_or(Assistant::Siri);
    let mut session = Session::new(platform, &contract, &mut coverage).await?;
    let mut stdout = std::io::stdout();
    writeln!(
        stdout,
        "{}",
        serde_json::json!({"ok": true, "document": {"variables": session.variables()}})
    )?;
    stdout.flush()?;
    // Steps come one at a time, so reading standard input in place is
    // enough; the server and the sync run on the runtime's other threads.
    for line in std::io::stdin().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let answer = match serde_json::from_str::<Step>(&line) {
            Ok(step) => match session.step(&step).await {
                Ok(record) => serde_json::json!({"ok": true, "document": record.document,
                                                 "request": record.request}),
                Err(error) => serde_json::json!({"ok": false, "error": error}),
            },
            Err(error) => serde_json::json!({"ok": false, "error": error.to_string()}),
        };
        writeln!(stdout, "{answer}")?;
        stdout.flush()?;
    }
    Ok(true)
}
