//! Parsing what the tools print. The fixtures in the tests are built from
//! the tools' own print statements (xtool's `AuthCommand.swift`,
//! `SDKCommand.swift`; usbipd-win's `list`; swift's `--version`).

use crate::versions;

/// `/proc/version` names Microsoft's kernel inside WSL.
pub fn is_wsl_kernel(proc_version: &str) -> bool {
    let lower = proc_version.to_ascii_lowercase();
    lower.contains("microsoft") || lower.contains("wsl")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OsRelease {
    pub id: String,
    pub version_id: String,
    pub pretty_name: String,
}

pub fn os_release(text: &str) -> OsRelease {
    let mut release = OsRelease {
        id: String::new(),
        version_id: String::new(),
        pretty_name: String::new(),
    };
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches('"').to_string();
        match key.trim() {
            "ID" => release.id = value,
            "VERSION_ID" => release.version_id = value,
            "PRETTY_NAME" => release.pretty_name = value,
            _ => {}
        }
    }
    release
}

impl OsRelease {
    pub fn is_ubuntu(&self) -> bool {
        self.id == "ubuntu"
    }

    /// `(major, minor)` of `VERSION_ID`, e.g. `(24, 4)`.
    pub fn version(&self) -> Option<(u32, u32)> {
        let (major, minor) = self.version_id.split_once('.')?;
        Some((major.parse().ok()?, minor.parse().ok()?))
    }

    /// The FUSE 2 library the xtool AppImage needs: renamed `libfuse2t64`
    /// in Ubuntu 24.04.
    pub fn fuse_package(&self) -> &'static str {
        match self.version() {
            Some(v) if v >= (24, 4) => "libfuse2t64",
            _ => "libfuse2",
        }
    }

    /// The packages swift.org lists for its Linux toolchain on this
    /// release.
    pub fn swift_dependencies(&self) -> &'static [&'static str] {
        match self.version() {
            Some(v) if v >= (24, 4) => &[
                "binutils",
                "gnupg2",
                "libc6-dev",
                "libcurl4-openssl-dev",
                "libedit2",
                "libgcc-13-dev",
                "libncurses-dev",
                "libpython3-dev",
                "libsqlite3-0",
                "libstdc++-13-dev",
                "libxml2-dev",
                "libz3-dev",
                "pkg-config",
                "tzdata",
                "unzip",
                "zlib1g-dev",
            ],
            _ => &[
                "binutils",
                "gnupg2",
                "libc6-dev",
                "libcurl4-openssl-dev",
                "libedit2",
                "libgcc-11-dev",
                "libpython3-dev",
                "libsqlite3-0",
                "libstdc++-11-dev",
                "libxml2-dev",
                "libz3-dev",
                "pkg-config",
                "tzdata",
                "unzip",
                "zlib1g-dev",
            ],
        }
    }
}

/// `dpkg-query -W -f '${Status}' <pkg>` says `install ok installed`.
pub fn dpkg_installed(status: &str) -> bool {
    status.trim() == "install ok installed"
}

/// `swift --version`: `Swift version 6.4 (swift-6.4-RELEASE)` (with an
/// `Apple ` prefix on macOS). Returns `6.4`.
pub fn swift_version(output: &str) -> Option<String> {
    let line = output.lines().find(|l| l.contains("Swift version "))?;
    let rest = line.split("Swift version ").nth(1)?;
    let version = rest.split_whitespace().next()?;
    Some(version.to_string())
}

/// Whether `found` is the pinned `wanted` release or a patch of it
/// (`6.4` wanted, `6.4` or `6.4.1` found).
pub fn version_matches(found: &str, wanted: &str) -> bool {
    found == wanted || found.starts_with(&format!("{wanted}."))
}

/// `node --version`: `v22.23.5`.
pub fn node_major(output: &str) -> Option<u32> {
    output
        .trim()
        .strip_prefix('v')?
        .split('.')
        .next()?
        .parse()
        .ok()
}

/// What `xtool auth status` says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthStatus {
    LoggedOut,
    /// API-key mode.
    Key {
        key_id: String,
        issuer_id: String,
    },
    /// Password mode.
    Password {
        apple_id: String,
        team_id: String,
        expiry: String,
    },
    /// Logged in, in a form this tool does not know.
    Other(String),
}

pub fn auth_status(output: &str) -> AuthStatus {
    let text = output.trim();
    if !text.starts_with("Logged in.") {
        return AuthStatus::LoggedOut;
    }
    let field = |name: &str| {
        text.lines()
            .find_map(|line| line.trim().strip_prefix(&format!("- {name}: ")))
            .map(|v| v.trim().to_string())
    };
    if let (Some(key_id), Some(issuer_id)) = (field("ASC key ID"), field("Issuer ID")) {
        return AuthStatus::Key { key_id, issuer_id };
    }
    if let Some(apple_id) = field("Apple ID") {
        return AuthStatus::Password {
            apple_id,
            team_id: field("Team ID").unwrap_or_default(),
            expiry: field("Token expiry").unwrap_or_default(),
        };
    }
    AuthStatus::Other(text.lines().skip(1).collect::<Vec<_>>().join("; "))
}

impl AuthStatus {
    pub fn summary(&self) -> String {
        match self {
            AuthStatus::LoggedOut => "logged out".into(),
            AuthStatus::Key { key_id, .. } => format!("API key {key_id}"),
            AuthStatus::Password {
                apple_id, team_id, ..
            } => format!("Apple ID {apple_id}, team {team_id}"),
            AuthStatus::Other(text) => format!("logged in ({text})"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SdkFlavor {
    Slim,
    Normal,
    Legacy,
    Unknown,
}

/// What `xtool sdk status` says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SdkStatus {
    NotInstalled,
    Installed {
        path: String,
        flavor: SdkFlavor,
        version: String,
    },
}

pub fn sdk_status(output: &str) -> SdkStatus {
    if !output.contains("Darwin SDK is installed") {
        return SdkStatus::NotInstalled;
    }
    let field = |name: &str| {
        output
            .lines()
            .find_map(|line| line.trim().strip_prefix(&format!("{name}: ")))
            .map(|v| v.trim().to_string())
            .unwrap_or_default()
    };
    let flavor = match field("Flavor").as_str() {
        "slim" => SdkFlavor::Slim,
        "normal" => SdkFlavor::Normal,
        "legacy" => SdkFlavor::Legacy,
        _ => SdkFlavor::Unknown,
    };
    SdkStatus::Installed {
        path: field("Path"),
        flavor,
        version: field("Version"),
    }
}

impl SdkStatus {
    /// As xtool's `isUpToDate`: the version string equals the one the
    /// pinned xtool builds.
    pub fn is_up_to_date(&self) -> bool {
        matches!(self, SdkStatus::Installed { version, .. } if version == versions::XTOOL_SDK_VERSION)
    }
}

/// One row of `usbipd list`'s "Connected" section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsbDevice {
    pub bus_id: String,
    pub vid_pid: String,
    pub description: String,
    pub state: String,
}

impl UsbDevice {
    pub fn is_apple(&self) -> bool {
        self.vid_pid.to_ascii_lowercase().starts_with("05ac:")
    }

    pub fn is_attached(&self) -> bool {
        self.state.starts_with("Attached")
    }

    /// Shared (bound) or attached: `bind` is done.
    pub fn is_shared(&self) -> bool {
        self.state.starts_with("Shared") || self.is_attached()
    }
}

/// The devices `usbipd list` shows as connected. Columns are separated by
/// two or more spaces.
pub fn usbipd_list(output: &str) -> Vec<UsbDevice> {
    let mut devices = Vec::new();
    let mut in_connected = false;
    for line in output.lines() {
        let trimmed = line.trim_end();
        if trimmed.starts_with("Connected:") {
            in_connected = true;
            continue;
        }
        if trimmed.ends_with(':') && !trimmed.starts_with(' ') {
            in_connected = false;
            continue;
        }
        if !in_connected || trimmed.is_empty() || trimmed.starts_with("BUSID") {
            continue;
        }
        let columns: Vec<&str> = split_columns(trimmed);
        if columns.len() < 3 || !crate::validate::bus_id(columns[0]) {
            continue;
        }
        let (description, state) = if columns.len() >= 4 {
            (
                columns[2..columns.len() - 1].join("  "),
                columns[columns.len() - 1],
            )
        } else {
            (columns[2].to_string(), "")
        };
        devices.push(UsbDevice {
            bus_id: columns[0].to_string(),
            vid_pid: columns[1].to_string(),
            description,
            state: state.to_string(),
        });
    }
    devices
}

fn split_columns(line: &str) -> Vec<&str> {
    let mut columns = Vec::new();
    let mut start = None;
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b' ' && bytes.get(i + 1) == Some(&b' ') {
            if let Some(s) = start.take() {
                columns.push(&line[s..i]);
            }
            while i < bytes.len() && bytes[i] == b' ' {
                i += 1;
            }
            continue;
        }
        if start.is_none() {
            start = Some(i);
        }
        i += 1;
    }
    if let Some(s) = start {
        columns.push(&line[s..]);
    }
    columns
        .into_iter()
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .collect()
}

/// `idevice_id -l`: one UDID a line.
pub fn idevice_ids(output: &str) -> Vec<String> {
    output
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && l.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-'))
        .map(str::to_string)
        .collect()
}

/// `SHASUMS256.txt`: the checksum listed for `file`.
pub fn shasums_entry(text: &str, file: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let (sum, name) = line.split_once(char::is_whitespace)?;
        let name = name.trim().trim_start_matches('*');
        (name == file).then(|| sum.to_ascii_lowercase())
    })
}

/// A rustup `.sha256` file: `<hex> *./rustup-init`.
pub fn sha256_file(text: &str) -> Option<String> {
    let sum = text.split_whitespace().next()?;
    (sum.len() == 64 && sum.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| sum.to_ascii_lowercase())
}

/// The SHA-256 GitHub records for the release asset named `asset`, from
/// the releases API's JSON (`"digest": "sha256:<hex>"`).
pub fn github_asset_digest(json: &str, asset: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let assets = value.get("assets")?.as_array()?;
    let entry = assets
        .iter()
        .find(|a| a.get("name").and_then(|n| n.as_str()) == Some(asset))?;
    let digest = entry.get("digest")?.as_str()?;
    let hex = digest.strip_prefix("sha256:")?;
    (hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| hex.to_ascii_lowercase())
}

/// gpg's `--status-fd` output: the primary-key fingerprint of a good
/// signature (`[GNUPG:] VALIDSIG <fpr> … <primary fpr>`).
pub fn gpg_valid_signature(status: &str) -> Option<String> {
    let line = status
        .lines()
        .find(|l| l.starts_with("[GNUPG:] VALIDSIG "))?;
    let fields: Vec<&str> = line.split_whitespace().collect();
    // [GNUPG:] VALIDSIG fpr date ts expire version reserved pk-algo hash-algo class primary-fpr
    let primary = fields.get(11).or_else(|| fields.get(2))?;
    Some(primary.to_ascii_uppercase())
}

/// The package names in a swiftly post-install script's
/// `apt-get … install …` lines (it lists what the toolchain needs).
pub fn apt_packages_in_script(script: &str) -> Vec<String> {
    let mut packages = Vec::new();
    for line in script.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        let Some(apt) = words.iter().position(|w| *w == "apt-get" || *w == "apt") else {
            continue;
        };
        let Some(install) = words[apt..].iter().position(|w| *w == "install") else {
            continue;
        };
        for word in &words[apt + install + 1..] {
            if matches!(*word, "&&" | "||" | ";" | "|") || word.ends_with(';') {
                break;
            }
            if word.starts_with('-') || *word == "\\" {
                continue;
            }
            if word
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'+' | b'.'))
            {
                packages.push((*word).to_string());
            }
        }
    }
    packages.sort();
    packages.dedup();
    packages
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wsl_detection() {
        assert!(is_wsl_kernel(
            "Linux version 6.6.87.2-microsoft-standard-WSL2 (root@x) (gcc) #1 SMP"
        ));
        assert!(!is_wsl_kernel(
            "Linux version 6.8.0-49-generic (buildd@lcy02)"
        ));
    }

    #[test]
    fn os_releases() {
        let noble = os_release(
            "PRETTY_NAME=\"Ubuntu 24.04.1 LTS\"\nNAME=\"Ubuntu\"\nVERSION_ID=\"24.04\"\nID=ubuntu\nID_LIKE=debian\n",
        );
        assert!(noble.is_ubuntu());
        assert_eq!(noble.version(), Some((24, 4)));
        assert_eq!(noble.fuse_package(), "libfuse2t64");
        assert!(noble.swift_dependencies().contains(&"libstdc++-13-dev"));
        let jammy = os_release("ID=ubuntu\nVERSION_ID=\"22.04\"\n");
        assert_eq!(jammy.fuse_package(), "libfuse2");
        assert!(jammy.swift_dependencies().contains(&"libstdc++-11-dev"));
        assert!(!os_release("ID=debian\nVERSION_ID=\"12\"").is_ubuntu());
    }

    #[test]
    fn dpkg() {
        assert!(dpkg_installed("install ok installed"));
        assert!(!dpkg_installed("deinstall ok config-files"));
        assert!(!dpkg_installed(""));
    }

    #[test]
    fn swift_versions() {
        // xtool's guide: "Swift version 6.4 (swift-6.4-RELEASE)".
        let linux = "Swift version 6.4 (swift-6.4-RELEASE)\nTarget: x86_64-unknown-linux-gnu\n";
        assert_eq!(swift_version(linux).as_deref(), Some("6.4"));
        let mac = "Apple Swift version 6.4.1 (swiftlang-6.4.1.1.1 clang-1700)\nTarget: arm64-apple-macosx15.0";
        assert_eq!(swift_version(mac).as_deref(), Some("6.4.1"));
        assert_eq!(swift_version("bash: swift: command not found"), None);
        assert!(version_matches("6.4", "6.4"));
        assert!(version_matches("6.4.1", "6.4"));
        assert!(!version_matches("6.40", "6.4"));
        assert!(!version_matches("6.3.2", "6.4"));
    }

    #[test]
    fn node_versions() {
        assert_eq!(node_major("v22.23.5\n"), Some(22));
        assert_eq!(node_major("garbage"), None);
    }

    #[test]
    fn xtool_auth_status() {
        // AuthStatusCommand: "Logged out", or "Logged in.\n\(token)" with
        // AuthToken.description.
        assert_eq!(auth_status("Logged out\n"), AuthStatus::LoggedOut);
        assert_eq!(
            auth_status("Logged in.\n- ASC key ID: 2X9R4HXF34\n- Issuer ID: 57246542-96fe-1a63-e053-0824d011072a\n"),
            AuthStatus::Key {
                key_id: "2X9R4HXF34".into(),
                issuer_id: "57246542-96fe-1a63-e053-0824d011072a".into()
            }
        );
        assert_eq!(
            auth_status("Logged in.\n- Apple ID: me@example.com\n- Team ID: ABCDE12345\n- Token expiry: 12/1/2026, 10:00 AM\n"),
            AuthStatus::Password {
                apple_id: "me@example.com".into(),
                team_id: "ABCDE12345".into(),
                expiry: "12/1/2026, 10:00 AM".into()
            }
        );
        assert!(matches!(
            auth_status("Logged in.\n- Something: x"),
            AuthStatus::Other(_)
        ));
    }

    #[test]
    fn xtool_sdk_status() {
        // DevSDKStatusCommand's prints.
        assert_eq!(sdk_status("Not installed\n"), SdkStatus::NotInstalled);
        let current = format!(
            "Darwin SDK is installed\n  Path: /home/ada/.swiftpm/swift-sdks/darwin.artifactbundle\n  Flavor: normal\n  Version: {}\n",
            versions::XTOOL_SDK_VERSION
        );
        let status = sdk_status(&current);
        assert_eq!(
            status,
            SdkStatus::Installed {
                path: "/home/ada/.swiftpm/swift-sdks/darwin.artifactbundle".into(),
                flavor: SdkFlavor::Normal,
                version: versions::XTOOL_SDK_VERSION.into()
            }
        );
        assert!(status.is_up_to_date());
        let old = sdk_status(
            "Darwin SDK is installed\n  Path: /x\n  Flavor: slim\n  Version: epoch=3,darwinTools=1.0.0,oam=1.0.0\n",
        );
        assert!(!old.is_up_to_date());
        assert!(matches!(
            old,
            SdkStatus::Installed {
                flavor: SdkFlavor::Slim,
                ..
            }
        ));
        let legacy = sdk_status(
            "Darwin SDK is installed\n  Path: /x\n  Flavor: legacy\n  Version: develop\n",
        );
        assert!(matches!(
            legacy,
            SdkStatus::Installed {
                flavor: SdkFlavor::Legacy,
                ..
            }
        ));
    }

    #[test]
    fn usbipd_lists() {
        // usbipd-win 4.x `usbipd list`.
        let output = "Connected:\n\
BUSID  VID:PID    DEVICE                                                        STATE\n\
1-1    05ac:12a8  Apple Mobile Device USB Composite Device                      Not shared\n\
2-3    8087:0026  Intel(R) Wireless Bluetooth(R)                                Not shared\n\
2-4    05ac:12a8  Apple Mobile Device USB Device, Apple Mobile Device Ethernet  Attached\n\
\n\
Persisted:\n\
GUID                                  DEVICE\n\
a1b2c3d4-0000-0000-0000-000000000000  USB Mass Storage Device\n";
        let devices = usbipd_list(output);
        assert_eq!(devices.len(), 3);
        assert_eq!(devices[0].bus_id, "1-1");
        assert!(devices[0].is_apple());
        assert_eq!(devices[0].state, "Not shared");
        assert!(!devices[0].is_shared());
        assert!(!devices[1].is_apple());
        assert_eq!(
            devices[2].description,
            "Apple Mobile Device USB Device, Apple Mobile Device Ethernet"
        );
        assert!(devices[2].is_attached());
        assert!(usbipd_list("").is_empty());
        let shared = usbipd_list("Connected:\nBUSID  VID:PID    DEVICE  STATE\n1-4    05AC:12A8  iPhone  Shared (forced)\n");
        assert!(shared[0].is_apple() && shared[0].is_shared() && !shared[0].is_attached());
    }

    #[test]
    fn idevice_lists() {
        assert_eq!(
            idevice_ids("00008110-001A2B3C4D5E801E\n\n"),
            vec!["00008110-001A2B3C4D5E801E".to_string()]
        );
        assert!(idevice_ids("ERROR: Unable to retrieve device list!").is_empty());
    }

    #[test]
    fn checksum_files() {
        let shasums = "2babb9eb  node-v22.23.5-linux-arm64.tar.xz\ncdeb8670  node-v22.23.5-linux-x64.tar.xz\n";
        assert_eq!(
            shasums_entry(shasums, "node-v22.23.5-linux-x64.tar.xz").as_deref(),
            Some("cdeb8670")
        );
        assert_eq!(shasums_entry(shasums, "node-v22.23.5-linux-x64.tar"), None);
        let sum =
            "b1c8cb14e30889e7e2999e6b69e62d9e9b9ffc0f4df8180564d306d9680e6c55 *./rustup-init\n";
        assert_eq!(sha256_file(sum).as_deref(), Some(&sum[..64]));
        assert_eq!(sha256_file("nope"), None);
    }

    #[test]
    fn github_digests() {
        let hex = "ab".repeat(32);
        let json = format!(
            r#"{{"tag_name":"1.24.0","assets":[{{"name":"xtool-x86_64.AppImage","digest":"sha256:{hex}"}},{{"name":"xtool-x86_64.AppImage.zsync","digest":"sha256:{}"}}]}}"#,
            "cd".repeat(32)
        );
        assert_eq!(
            github_asset_digest(&json, "xtool-x86_64.AppImage"),
            Some(hex)
        );
        assert_eq!(github_asset_digest(&json, "xtool-aarch64.AppImage"), None);
        assert_eq!(github_asset_digest("{}", "x"), None);
    }

    #[test]
    fn gpg_status() {
        let status = "[GNUPG:] NEWSIG\n[GNUPG:] GOODSIG F167DF1ACF9CE069 Swift Automatic Signing Key #4\n[GNUPG:] VALIDSIG 0123456789ABCDEF0123456789ABCDEF01234567 2026-10-01 1790000000 0 4 0 1 10 00 E813C892820A6FA13755B268F167DF1ACF9CE069\n";
        assert_eq!(
            gpg_valid_signature(status).as_deref(),
            Some("E813C892820A6FA13755B268F167DF1ACF9CE069")
        );
        assert_eq!(gpg_valid_signature("[GNUPG:] BADSIG x"), None);
    }

    #[test]
    fn swiftly_post_install() {
        let script = "#!/bin/sh\napt-get -y install libstdc++-13-dev libz3-dev && echo done\n";
        assert_eq!(
            apt_packages_in_script(script),
            vec!["libstdc++-13-dev".to_string(), "libz3-dev".to_string()]
        );
        assert!(apt_packages_in_script("echo hi; rm -rf /").is_empty());
    }
}
