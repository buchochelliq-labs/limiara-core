//! SHA-256 checks of downloads.

use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

pub fn sha256_hex(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path).map_err(|e| Error::file(path, &e))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 16];
    loop {
        let read = file.read(&mut buffer).map_err(|e| Error::file(path, &e))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(DIGITS[(b >> 4) as usize] as char);
        out.push(DIGITS[(b & 0xf) as usize] as char);
    }
    out
}

/// Check `path` against every expected checksum (all must agree). On a
/// mismatch the file is deleted. With no checksum at all, nothing is
/// trusted.
pub fn verify(path: &Path, what: &str, expected: &[&str]) -> Result<String> {
    if expected.is_empty() {
        let _ = std::fs::remove_file(path);
        return Err(Error::ChecksumUnavailable { what: what.into() });
    }
    let actual = sha256_hex(path)?;
    if expected.iter().all(|sum| sum.eq_ignore_ascii_case(&actual)) {
        Ok(actual)
    } else {
        let _ = std::fs::remove_file(path);
        Err(Error::ChecksumMismatch { what: what.into() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[test]
    fn known_digest() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("abc");
        std::fs::write(&file, b"abc").unwrap();
        assert_eq!(sha256_hex(&file).unwrap(), ABC);
    }

    #[test]
    fn verifies_and_deletes_on_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("abc");
        std::fs::write(&file, b"abc").unwrap();
        assert_eq!(
            verify(&file, "abc", &[ABC, &ABC.to_uppercase()]).unwrap(),
            ABC
        );
        assert!(file.exists());
        let wrong = "0".repeat(64);
        let err = verify(&file, "abc", &[ABC, &wrong]).unwrap_err();
        assert_eq!(err.code(), "LS021_CHECKSUM_MISMATCH");
        assert!(!file.exists());
    }

    #[test]
    fn no_checksum_no_trust() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("abc");
        std::fs::write(&file, b"abc").unwrap();
        assert_eq!(
            verify(&file, "abc", &[]).unwrap_err().code(),
            "LS022_CHECKSUM_UNAVAILABLE"
        );
        assert!(!file.exists());
    }
}
