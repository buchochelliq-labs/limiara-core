//! Errors with stable codes. A message is a plain sentence: it never holds
//! a password, a code, a key's contents or a command's full arguments.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("this tool runs inside WSL (Ubuntu) on Windows: {0}")]
    NotWsl(String),
    #[error("this Linux distribution is not supported: {0}")]
    UnsupportedDistro(String),
    #[error("this machine's architecture is not supported (xtool ships x86_64 and aarch64)")]
    UnsupportedArch,
    #[error("'{program}' could not be started: {reason}")]
    Spawn { program: String, reason: String },
    #[error("'{program}' failed ({status})")]
    CommandFailed { program: String, status: String },
    #[error("the download of {what} failed")]
    Download { what: String },
    #[error("the checksum of {what} does not match the pinned value; the file was deleted")]
    ChecksumMismatch { what: String },
    #[error("no checksum is available to verify {what}, so it was not installed")]
    ChecksumUnavailable { what: String },
    #[error("the signature of {what} could not be verified with swift.org's keys")]
    SignatureInvalid { what: String },
    #[error("{0}")]
    InvalidInput(String),
    #[error("the key file is not usable: {0}")]
    InvalidKey(String),
    #[error("the key must not be kept at {path}: {reason}")]
    UnsafeKeyLocation { path: PathBuf, reason: String },
    #[error("{path}: {reason}")]
    File { path: PathBuf, reason: String },
    #[error("the state file {path} could not be read or written: {reason}")]
    State { path: PathBuf, reason: String },
    #[error("cancelled")]
    Cancelled,
    #[error("the terminal could not be used: {0}")]
    Terminal(String),
    #[error("{0}")]
    NotReady(String),
}

impl Error {
    /// The stable code printed with every error.
    pub fn code(&self) -> &'static str {
        match self {
            Error::NotWsl(_) => "LS001_NOT_WSL",
            Error::UnsupportedDistro(_) => "LS002_UNSUPPORTED_DISTRO",
            Error::UnsupportedArch => "LS003_UNSUPPORTED_ARCH",
            Error::Spawn { .. } => "LS010_SPAWN",
            Error::CommandFailed { .. } => "LS011_COMMAND_FAILED",
            Error::Download { .. } => "LS020_DOWNLOAD",
            Error::ChecksumMismatch { .. } => "LS021_CHECKSUM_MISMATCH",
            Error::ChecksumUnavailable { .. } => "LS022_CHECKSUM_UNAVAILABLE",
            Error::SignatureInvalid { .. } => "LS023_SIGNATURE_INVALID",
            Error::InvalidInput(_) => "LS030_INVALID_INPUT",
            Error::InvalidKey(_) => "LS031_INVALID_KEY",
            Error::UnsafeKeyLocation { .. } => "LS032_UNSAFE_KEY_LOCATION",
            Error::File { .. } => "LS040_FILE",
            Error::State { .. } => "LS041_STATE",
            Error::Cancelled => "LS050_CANCELLED",
            Error::Terminal(_) => "LS051_TERMINAL",
            Error::NotReady(_) => "LS060_NOT_READY",
        }
    }

    pub fn file(path: impl Into<PathBuf>, error: &std::io::Error) -> Error {
        Error::File {
            path: path.into(),
            reason: io_reason(error),
        }
    }
}

/// An I/O error as a plain phrase, without the OS's own formatting of
/// paths or contents.
pub fn io_reason(error: &std::io::Error) -> String {
    use std::io::ErrorKind;
    match error.kind() {
        ErrorKind::NotFound => "not found".into(),
        ErrorKind::PermissionDenied => "permission denied".into(),
        ErrorKind::AlreadyExists => "already exists".into(),
        _ => error.kind().to_string(),
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_unique() {
        let all = [
            Error::NotWsl(String::new()),
            Error::UnsupportedDistro(String::new()),
            Error::UnsupportedArch,
            Error::Spawn {
                program: String::new(),
                reason: String::new(),
            },
            Error::CommandFailed {
                program: String::new(),
                status: String::new(),
            },
            Error::Download {
                what: String::new(),
            },
            Error::ChecksumMismatch {
                what: String::new(),
            },
            Error::ChecksumUnavailable {
                what: String::new(),
            },
            Error::SignatureInvalid {
                what: String::new(),
            },
            Error::InvalidInput(String::new()),
            Error::InvalidKey(String::new()),
            Error::UnsafeKeyLocation {
                path: PathBuf::new(),
                reason: String::new(),
            },
            Error::File {
                path: PathBuf::new(),
                reason: String::new(),
            },
            Error::State {
                path: PathBuf::new(),
                reason: String::new(),
            },
            Error::Cancelled,
            Error::Terminal(String::new()),
            Error::NotReady(String::new()),
        ];
        let mut codes: Vec<_> = all.iter().map(Error::code).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), all.len());
    }
}
