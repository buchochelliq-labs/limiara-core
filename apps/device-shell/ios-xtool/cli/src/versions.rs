//! Every pinned version, checksum and download location, in one place.
//!
//! Updating a pin is a reviewed change to this file; README.md ("Updating
//! the pins") lists how each value was obtained.

/// The repository this tool builds, and the branch that has
/// `apps/device-shell/ios-xtool`.
pub const REPO_URL: &str = "https://github.com/buchochelliq-labs/limiara-core";
pub const DEFAULT_BRANCH: &str = "claude/xtool-sideload-spike";
pub const MAIN_BRANCH: &str = "main";
/// The project directory inside the repository.
pub const XTOOL_PROJECT_DIR: &str = "apps/device-shell/ios-xtool";

/// The Rust toolchain the repository pins in `rust-toolchain.toml` (a test
/// checks they agree).
pub const RUST_TOOLCHAIN: &str = "1.95.0";
pub const IOS_RUST_TARGET: &str = "aarch64-apple-ios";

/// rustup-init, when rustup is missing. `.sha256` files are published next
/// to each binary under static.rust-lang.org/rustup/archive/<version>/.
pub const RUSTUP_VERSION: &str = "1.29.3";
pub const RUSTUP_SHA256_X86_64: &str =
    "b1c8cb14e30889e7e2999e6b69e62d9e9b9ffc0f4df8180564d306d9680e6c55";
pub const RUSTUP_SHA256_AARCH64: &str =
    "9f608c142e0e38a78957d0661ba1dfaf0b6fb37184cfaa4b9a0aa57af6184282";

/// Node.js, from the official tarball; checked against the pinned SHA-256
/// and the release's own SHASUMS256.txt.
pub const NODE_VERSION: &str = "22.23.5";
pub const NODE_SHA256_X64: &str =
    "cdeb8670a0c81f51b4dc4ac61919f9b1ad917b304bf8221eb939570663d240f8";
pub const NODE_SHA256_ARM64: &str =
    "2babb9eb5264860d540849f4b064a6fbdebcdc323657d9d382ce23d313fd5e09";
/// pnpm, through corepack (the repository's `packageManager`).
pub const PNPM_VERSION: &str = "9.12.0";

/// The Swift toolchain xtool's Linux guide names, installed with swiftly.
pub const SWIFT_VERSION: &str = "6.4";
/// swiftly, swift.org's installer. The tarball is checked against its
/// detached signature with swift.org's keys before it runs.
pub const SWIFTLY_BASE_URL: &str = "https://download.swift.org/swiftly/linux";
pub const SWIFT_KEYS_URL: &str = "https://www.swift.org/keys/all-keys.asc";
/// Primary-key fingerprints that may sign swiftly: the "Swift 6.x Release
/// Signing Key" and the "Swift Automatic Signing Key #4", as published in
/// all-keys.asc. A signature by any other key is refused.
pub const SWIFT_SIGNING_FINGERPRINTS: &[&str] = &[
    "52BB7E3DE28A71BE22EC05FFEF80A866B47A981F",
    "E813C892820A6FA13755B268F167DF1ACF9CE069",
];

/// xtool's release. xtool publishes no checksum file, so the AppImage is
/// checked against the SHA-256 GitHub records for the release asset (the
/// `digest` of the asset in the releases API), and against the pin below
/// when one is recorded.
pub const XTOOL_VERSION: &str = "1.24.0";
/// SHA-256 of `xtool-<arch>.AppImage` for [`XTOOL_VERSION`]. `None` until
/// someone with access to the release records it (README.md, "Updating the
/// pins"); until then the GitHub asset digest is the reference, and the
/// user is shown it and asked to confirm.
pub const XTOOL_SHA256_X86_64: Option<&str> = None;
pub const XTOOL_SHA256_AARCH64: Option<&str> = None;
/// The Darwin SDK version string `xtool sdk status` prints for an SDK
/// that xtool [`XTOOL_VERSION`] considers up to date
/// (`SDKBuilder.currentSDKVersion`).
pub const XTOOL_SDK_VERSION: &str = "epoch=4,darwinTools=1.1.0,oam=1.4.1";
/// The Xcode release xtool's Linux guide names.
pub const XCODE_VERSION: &str = "27.1";
pub const XCODE_DOWNLOAD_URL: &str = "https://developer.apple.com/download/all/?q=Xcode";

/// Links from xtool's Linux guide.
pub const XTOOL_LINUX_GUIDE: &str =
    "https://github.com/xtool-org/xtool/blob/main/Documentation/xtool.docc/Installation-Linux.md";
pub const MUX_ERROR_WORKAROUND: &str =
    "https://github.com/xtool-org/xtool/issues/19#issuecomment-2898986718";
pub const USBIPD_GUIDE: &str = "https://learn.microsoft.com/en-us/windows/wsl/connect-usb";
pub const ASC_KEYS_GUIDE: &str = "https://developer.apple.com/documentation/appstoreconnectapi/creating-api-keys-for-app-store-connect-api";

/// The machine architecture, as the downloads name it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arch {
    X86_64,
    Aarch64,
}

impl Arch {
    pub fn current() -> Option<Arch> {
        match std::env::consts::ARCH {
            "x86_64" => Some(Arch::X86_64),
            "aarch64" => Some(Arch::Aarch64),
            _ => None,
        }
    }

    /// `uname -m`.
    pub fn uname(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86_64",
            Arch::Aarch64 => "aarch64",
        }
    }

    pub fn node(self) -> &'static str {
        match self {
            Arch::X86_64 => "x64",
            Arch::Aarch64 => "arm64",
        }
    }

    pub fn node_sha256(self) -> &'static str {
        match self {
            Arch::X86_64 => NODE_SHA256_X64,
            Arch::Aarch64 => NODE_SHA256_ARM64,
        }
    }

    pub fn rustup_sha256(self) -> &'static str {
        match self {
            Arch::X86_64 => RUSTUP_SHA256_X86_64,
            Arch::Aarch64 => RUSTUP_SHA256_AARCH64,
        }
    }

    pub fn xtool_sha256(self) -> Option<&'static str> {
        match self {
            Arch::X86_64 => XTOOL_SHA256_X86_64,
            Arch::Aarch64 => XTOOL_SHA256_AARCH64,
        }
    }
}

pub fn node_tarball(arch: Arch) -> String {
    format!("node-v{NODE_VERSION}-linux-{}.tar.xz", arch.node())
}

pub fn node_url(file: &str) -> String {
    format!("https://nodejs.org/dist/v{NODE_VERSION}/{file}")
}

pub fn rustup_url(arch: Arch) -> String {
    format!(
        "https://static.rust-lang.org/rustup/archive/{RUSTUP_VERSION}/{}-unknown-linux-gnu/rustup-init",
        arch.uname()
    )
}

pub fn xtool_asset(arch: Arch) -> String {
    format!("xtool-{}.AppImage", arch.uname())
}

pub fn xtool_url(arch: Arch) -> String {
    format!(
        "https://github.com/xtool-org/xtool/releases/download/{XTOOL_VERSION}/{}",
        xtool_asset(arch)
    )
}

pub fn xtool_release_api() -> String {
    format!("https://api.github.com/repos/xtool-org/xtool/releases/tags/{XTOOL_VERSION}")
}

pub fn swiftly_url(arch: Arch) -> String {
    format!("{SWIFTLY_BASE_URL}/swiftly-{}.tar.gz", arch.uname())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_toolchain_matches_the_repository() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../rust-toolchain.toml"
        );
        let text = std::fs::read_to_string(path).unwrap();
        assert!(
            text.contains(&format!("channel = \"{RUST_TOOLCHAIN}\"")),
            "rust-toolchain.toml and versions.rs disagree"
        );
    }

    #[test]
    fn pnpm_matches_the_repository() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../../package.json");
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains(&format!("\"packageManager\": \"pnpm@{PNPM_VERSION}\"")));
    }

    #[test]
    fn checksums_are_hex_sha256() {
        for sum in [
            NODE_SHA256_X64,
            NODE_SHA256_ARM64,
            RUSTUP_SHA256_X86_64,
            RUSTUP_SHA256_AARCH64,
        ] {
            assert_eq!(sum.len(), 64);
            assert!(sum.bytes().all(|b| b.is_ascii_hexdigit()));
        }
        for fpr in SWIFT_SIGNING_FINGERPRINTS {
            assert_eq!(fpr.len(), 40);
        }
    }

    #[test]
    fn urls_are_https_and_pinned() {
        for arch in [Arch::X86_64, Arch::Aarch64] {
            for url in [
                node_url(&node_tarball(arch)),
                rustup_url(arch),
                xtool_url(arch),
                swiftly_url(arch),
            ] {
                assert!(url.starts_with("https://"), "{url}");
            }
            assert!(xtool_url(arch).contains(XTOOL_VERSION));
            assert!(node_url(&node_tarball(arch)).contains(NODE_VERSION));
            assert!(rustup_url(arch).contains(RUSTUP_VERSION));
        }
    }
}
