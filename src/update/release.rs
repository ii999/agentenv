//! Release discovery, download, verification, and extraction.
//!
//! The release layout mirrors what `.github/workflows/release.yml` publishes:
//! one archive per target named `agentenv-<tag>-<target>.<ext>`, unpacking
//! to a directory of the same stem that holds the binary and `skills/`, plus
//! a `SHA256SUMS` file in `sha256sum` format covering every archive.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use semver::Version;
use sha2::{Digest, Sha256};

use crate::error::AppError;

use super::TARGET;

/// Where releases are published when `AGENTENV_RELEASE_BASE_URL` is unset.
pub const DEFAULT_BASE_URL: &str = "https://github.com/ii999/agentenv/releases";

const SUMS_FILE: &str = "SHA256SUMS";
const SUMS_LIMIT: u64 = 64 * 1024;
const ARCHIVE_LIMIT: u64 = 256 * 1024 * 1024;

#[cfg(windows)]
const ARCHIVE_EXTENSION: &str = "zip";
#[cfg(not(windows))]
const ARCHIVE_EXTENSION: &str = "tar.gz";

#[cfg(windows)]
const BINARY_NAME: &str = "agentenv.exe";
#[cfg(not(windows))]
const BINARY_NAME: &str = "agentenv";
#[cfg(windows)]
const SUDO_HELPER_NAME: &str = "agentenv-sudo-helper.exe";
#[cfg(not(windows))]
const SUDO_HELPER_NAME: &str = "agentenv-sudo-helper";
#[cfg(windows)]
const SSH_ASKPASS_NAME: &str = "agentenv-ssh-askpass.exe";
#[cfg(not(windows))]
const SSH_ASKPASS_NAME: &str = "agentenv-ssh-askpass";

/// The archive for this platform inside one release.
#[derive(Debug, Clone)]
pub struct Asset {
    pub name: String,
    /// Lowercase hex SHA-256 from `SHA256SUMS`.
    pub digest: String,
}

/// One published release, resolved for this platform.
#[derive(Debug, Clone)]
pub struct Release {
    pub tag: String,
    pub version: Version,
    pub asset: Asset,
}

/// The contents of an unpacked release archive.
#[derive(Debug)]
pub struct Extracted {
    pub binary: PathBuf,
    pub sudo_helper: PathBuf,
    pub ssh_askpass: PathBuf,
    /// The packaged skill directory, when the release ships one.
    pub skill: Option<PathBuf>,
}

pub struct Client {
    base_url: String,
    agent: ureq::Agent,
}

impl Client {
    pub fn new(base_url: &str) -> Self {
        let config = ureq::Agent::config_builder()
            .user_agent(format!("agentenv/{}", env!("CARGO_PKG_VERSION")))
            .timeout_connect(Some(Duration::from_secs(20)))
            .timeout_recv_response(Some(Duration::from_secs(60)))
            .build();
        Self {
            base_url: base_url.to_owned(),
            agent: ureq::Agent::new_with_config(config),
        }
    }

    /// Resolves the release for `tag`, or the latest release when `tag` is
    /// `None`, from its `SHA256SUMS` file.
    pub fn fetch(&self, tag: Option<&str>) -> Result<Release, AppError> {
        let url = match tag {
            Some(tag) => format!("{}/download/{tag}/{SUMS_FILE}", self.base_url),
            None => format!("{}/latest/download/{SUMS_FILE}", self.base_url),
        };
        let describe = || match tag {
            Some(tag) => format!("release {tag}"),
            None => "the latest release".to_owned(),
        };
        let mut response = self.agent.get(&url).call().map_err(|error| match error {
            ureq::Error::StatusCode(404) => AppError::Update(format!(
                "{} has no {SUMS_FILE} at {url}; check the release tag",
                describe()
            )),
            other => AppError::Update(format!(
                "cannot fetch {SUMS_FILE} for {}: {other}",
                describe()
            )),
        })?;
        let sums = response
            .body_mut()
            .with_config()
            .limit(SUMS_LIMIT)
            .read_to_string()
            .map_err(|error| {
                AppError::Update(format!(
                    "cannot read {SUMS_FILE} for {}: {error}",
                    describe()
                ))
            })?;
        parse_release(&sums, tag)
    }

    /// Downloads the release archive into `dir`, verifying its SHA-256 digest
    /// against the entry from `SHA256SUMS` before returning the path.
    pub fn download(&self, release: &Release, dir: &Path) -> Result<PathBuf, AppError> {
        let url = format!(
            "{}/download/{}/{}",
            self.base_url, release.tag, release.asset.name
        );
        let mut response = self.agent.get(&url).call().map_err(|error| {
            AppError::Update(format!("cannot download {}: {error}", release.asset.name))
        })?;
        let destination = dir.join(&release.asset.name);
        let mut file = File::create(&destination).map_err(|error| {
            AppError::Update(format!("cannot create {}: {error}", destination.display()))
        })?;
        let mut reader = response
            .body_mut()
            .with_config()
            .limit(ARCHIVE_LIMIT)
            .reader();
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let read = reader.read(&mut buffer).map_err(|error| {
                AppError::Update(format!(
                    "download of {} failed: {error}",
                    release.asset.name
                ))
            })?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            file.write_all(&buffer[..read]).map_err(|error| {
                AppError::Update(format!("cannot write {}: {error}", destination.display()))
            })?;
        }
        file.flush().map_err(|error| {
            AppError::Update(format!("cannot write {}: {error}", destination.display()))
        })?;
        let actual = hex(&hasher.finalize());
        if actual != release.asset.digest {
            return Err(AppError::Update(format!(
                "checksum verification failed for {}: {SUMS_FILE} lists {} but the download hashes to {actual}",
                release.asset.name, release.asset.digest
            )));
        }
        Ok(destination)
    }
}

/// Finds this platform's asset in a `SHA256SUMS` document and derives the
/// release tag and version from its name.
fn parse_release(sums: &str, expected_tag: Option<&str>) -> Result<Release, AppError> {
    let suffix = format!("-{TARGET}.{ARCHIVE_EXTENSION}");
    let mut found = None;
    for line in sums.lines() {
        let mut fields = line.split_whitespace();
        let (Some(digest), Some(name)) = (fields.next(), fields.next()) else {
            continue;
        };
        let name = name.trim_start_matches('*');
        let Some(stem) = name.strip_prefix("agentenv-") else {
            continue;
        };
        let Some(tag) = stem.strip_suffix(suffix.as_str()) else {
            continue;
        };
        if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(AppError::Update(format!(
                "{SUMS_FILE} carries a malformed digest for {name}"
            )));
        }
        if found.is_some() {
            return Err(AppError::Update(format!(
                "{SUMS_FILE} lists more than one archive for {TARGET}"
            )));
        }
        found = Some((tag.to_owned(), name.to_owned(), digest.to_ascii_lowercase()));
    }
    let Some((tag, name, digest)) = found else {
        return Err(AppError::Update(format!(
            "{} ships no prebuilt binary for {TARGET}; build from source with 'cargo install --path .'",
            match expected_tag {
                Some(tag) => format!("release {tag}"),
                None => "the latest release".to_owned(),
            }
        )));
    };
    if let Some(expected) = expected_tag {
        if expected != tag {
            return Err(AppError::Update(format!(
                "{SUMS_FILE} for release {expected} names archive {name}, which belongs to {tag}"
            )));
        }
    }
    let version = Version::parse(tag.trim_start_matches('v')).map_err(|error| {
        AppError::Update(format!(
            "release tag {tag} is not a semantic version: {error}"
        ))
    })?;
    Ok(Release {
        tag,
        version,
        asset: Asset { name, digest },
    })
}

/// Unpacks a verified archive into `dir` and locates the binary and skill.
pub fn extract(archive: &Path, release: &Release, dir: &Path) -> Result<Extracted, AppError> {
    unpack(archive, dir)?;
    let root = dir.join(format!("agentenv-{}-{TARGET}", release.tag));
    let binary = root.join(BINARY_NAME);
    let sudo_helper = root.join(SUDO_HELPER_NAME);
    let ssh_askpass = root.join(SSH_ASKPASS_NAME);
    for (path, name) in [
        (&binary, BINARY_NAME),
        (&sudo_helper, SUDO_HELPER_NAME),
        (&ssh_askpass, SSH_ASKPASS_NAME),
    ] {
        if path.is_file() {
            continue;
        }
        return Err(AppError::Update(format!(
            "{} does not contain {}/{name}; install a complete release bundle",
            release.asset.name,
            root.file_name().unwrap_or_default().to_string_lossy()
        )));
    }
    let skill = root.join("skills").join("agentenv");
    let skill = skill.join("SKILL.md").is_file().then_some(skill);
    Ok(Extracted {
        binary,
        sudo_helper,
        ssh_askpass,
        skill,
    })
}

#[cfg(not(windows))]
fn unpack(archive: &Path, dir: &Path) -> Result<(), AppError> {
    let file = File::open(archive)
        .map_err(|error| AppError::Update(format!("cannot open {}: {error}", archive.display())))?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
    tar.set_preserve_permissions(true);
    tar.unpack(dir)
        .map_err(|error| AppError::Update(format!("cannot unpack {}: {error}", archive.display())))
}

#[cfg(windows)]
fn unpack(archive: &Path, dir: &Path) -> Result<(), AppError> {
    let file = File::open(archive)
        .map_err(|error| AppError::Update(format!("cannot open {}: {error}", archive.display())))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|error| AppError::Update(format!("cannot open {}: {error}", archive.display())))?;
    zip.extract(dir)
        .map_err(|error| AppError::Update(format!("cannot unpack {}: {error}", archive.display())))
}

/// Runs the extracted binary once and confirms it reports `version`, so a
/// mislabeled or broken build never replaces the running one.
pub fn verify_binary(binary: &Path, version: &Version) -> Result<(), AppError> {
    let output = Command::new(binary)
        .arg("--version")
        .output()
        .map_err(|error| {
            AppError::Update(format!(
                "cannot run the downloaded binary {}: {error}",
                binary.display()
            ))
        })?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let reported = stdout.trim().strip_prefix("agentenv ").map(str::trim);
    if !output.status.success() || reported != Some(version.to_string().as_str()) {
        return Err(AppError::Update(format!(
            "the downloaded binary reports '{}' instead of 'agentenv {version}'",
            stdout.trim()
        )));
    }
    Ok(())
}

/// Confirms that every executable in the archive belongs to the same build.
pub fn verify_bundle(extracted: &Extracted, version: &Version) -> Result<(), AppError> {
    verify_binary(&extracted.binary, version)?;
    verify_identity(
        &extracted.sudo_helper,
        &format!("agentenv-sudo-helper 1 {version}\n"),
    )?;
    verify_identity(
        &extracted.ssh_askpass,
        &format!("agentenv-ssh-askpass 1 {version}\n"),
    )
}

fn verify_identity(binary: &Path, expected: &str) -> Result<(), AppError> {
    let output = Command::new(binary)
        .arg("--identity")
        .output()
        .map_err(|error| {
            AppError::Update(format!(
                "cannot run downloaded companion {}: {error}",
                binary.display()
            ))
        })?;
    if !output.status.success() || output.stdout != expected.as_bytes() || !output.stderr.is_empty()
    {
        return Err(AppError::Update(format!(
            "downloaded companion {} has a missing or mismatched identity; install a complete matching release bundle",
            binary.file_name().unwrap_or_default().to_string_lossy()
        )));
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sums(tag: &str) -> String {
        let digest = "a".repeat(64);
        format!(
            "{digest}  agentenv-{tag}-{TARGET}.{ARCHIVE_EXTENSION}\n{digest}  agentenv-{tag}-other-target.tar.gz\n"
        )
    }

    #[test]
    fn parse_release_derives_tag_and_version_from_the_asset_name() {
        let release = parse_release(&sums("v1.2.3"), None).expect("parses");
        assert_eq!(release.tag, "v1.2.3");
        assert_eq!(release.version, Version::new(1, 2, 3));
        assert_eq!(release.asset.digest, "a".repeat(64));
    }

    #[test]
    fn parse_release_rejects_a_sums_file_from_another_tag() {
        let error = parse_release(&sums("v1.2.3"), Some("v9.9.9")).expect_err("mismatch");
        assert!(error.to_string().contains("v9.9.9"));
    }

    #[test]
    fn parse_release_reports_a_missing_platform_asset() {
        let error =
            parse_release("abc  agentenv-v1.0.0-nowhere.tar.gz\n", None).expect_err("missing");
        assert!(error.to_string().contains(TARGET));
    }
}
