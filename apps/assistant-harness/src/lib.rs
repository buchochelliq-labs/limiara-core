//! The reference test harness for the assistant bridge (ADR 0034 slice 4).
//!
//! It drives the `assistant.v1` interface with no phone: a workspace server
//! and a device that syncs from it into an encrypted SQLite replica
//! ([`world`]), a native plugin's `device.assistant` grant, and requests
//! sent as JSON through `call_json`, exactly as a Swift or Kotlin plugin
//! sends them. Sessions are scripts ([`script`]) under
//! `tests/assistant-harness/`; every request and answer is checked against
//! the contract file ([`schema`]), and the suite must reach every
//! operation, code and index update the contract lists ([`coverage`]).
//! Time moves only when a script moves the injectable clock ([`clock`]).

#![forbid(unsafe_code)]

pub mod clock;
pub mod coverage;
pub mod runner;
pub mod schema;
pub mod script;
pub mod world;

use platform_objects::Assistant;
use std::path::{Path, PathBuf};

/// The outcome of running scripts.
#[derive(Debug, Default)]
pub struct SuiteReport {
    pub passed: Vec<String>,
    pub failed: Vec<(String, String)>,
    pub transcripts: Vec<runner::Transcript>,
    /// What the suite did not reach, when coverage was checked.
    pub missing: Vec<String>,
    pub summary: String,
}

impl SuiteReport {
    pub fn ok(&self) -> bool {
        self.failed.is_empty() && self.missing.is_empty()
    }
}

/// Runs every script at `paths` (the suite when empty) for each assistant,
/// each in a world of its own. With `check_coverage`, what the scripts
/// together did not reach is a failure too.
pub async fn run(
    paths: &[PathBuf],
    assistants: &[Assistant],
    check_coverage: bool,
) -> anyhow::Result<SuiteReport> {
    let contract = schema::Contract::bundled()?;
    let mut files = Vec::new();
    if paths.is_empty() {
        files = script::discover(&script::suite_dir())?;
    }
    for path in paths {
        files.extend(script::discover(path)?);
    }
    anyhow::ensure!(!files.is_empty(), "no scripts found");
    let mut report = SuiteReport::default();
    for assistant in assistants {
        let mut coverage = coverage::Coverage::default();
        for file in &files {
            let script = script::Script::load(file)?;
            let label = format!("{} ({})", script.name, assistant.as_str());
            match runner::run_script(&script, *assistant, &contract, &mut coverage).await {
                Ok(transcript) => {
                    report.passed.push(label);
                    report.transcripts.push(transcript);
                }
                Err(failure) => report.failed.push((label, failure.to_string())),
            }
        }
        if check_coverage {
            report.missing.extend(
                coverage
                    .missing(&contract)
                    .into_iter()
                    .map(|missing| format!("{}: {missing}", assistant.as_str())),
            );
        }
        report.summary = coverage.summary();
    }
    Ok(report)
}

/// Writes what a native plugin's own tests replay: the contract file, the
/// scripts, and one transcript per script and assistant with the exact
/// request bytes and the answers the bridge gave.
pub fn export(report: &SuiteReport, scripts: &[PathBuf], out: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(out.join("scripts"))?;
    std::fs::create_dir_all(out.join("transcripts"))?;
    std::fs::write(
        out.join("assistant.v1.schema.json"),
        platform_device::assistant::SCHEMA,
    )?;
    for script in scripts {
        if let Some(name) = script.file_name() {
            std::fs::copy(script, out.join("scripts").join(name))?;
        }
    }
    for transcript in &report.transcripts {
        let name = format!("{}.{}.json", transcript.script, transcript.assistant);
        std::fs::write(
            out.join("transcripts").join(name),
            serde_json::to_string_pretty(transcript)?,
        )?;
    }
    Ok(())
}
