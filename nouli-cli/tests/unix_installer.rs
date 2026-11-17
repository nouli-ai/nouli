//! Execute the distributed POSIX installer with Rust-controlled tools and real archives.
#![cfg(unix)]
#[path = "support/child.rs"]
#[allow(dead_code, reason = "installer uses only the Cargo fixture path")]
mod child;
use std::{fs, path::PathBuf, process::Command};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let root =
            std::env::temp_dir().join(format!("nouli-sh-{}", nouli_core::identity::Id::generate()));
        fs::create_dir_all(root.join("bin")).unwrap();
        for tool in ["curl", "uname", "getconf"] {
            fs::copy(child::executable(), root.join("bin").join(tool)).unwrap();
        }
        let archive = fs::File::create(root.join("release.tar.gz")).unwrap();
        let mut archive = tar::Builder::new(flate2::write::GzEncoder::new(
            archive,
            flate2::Compression::fast(),
        ));
        archive
            .append_path_with_name(child::executable(), "nouli")
            .unwrap();
        archive.into_inner().unwrap().finish().unwrap();
        Self(root)
    }
    fn command(&self) -> Command {
        let digest = Command::new("shasum")
            .args(["-a", "256"])
            .arg(self.0.join("release.tar.gz"))
            .output()
            .unwrap();
        assert!(digest.status.success());
        let checksum = String::from_utf8(digest.stdout)
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .to_owned();
        let mut command = Command::new("sh");
        command
            .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../scripts/install.sh"))
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.0.join("bin").display(),
                    std::env::var("PATH").unwrap()
                ),
            )
            .env("TMPDIR", &self.0)
            .env("NOULI_INSTALL_DIR", self.0.join("destination with spaces"))
            .env("NOULI_VERSION", "latest")
            .env("FIXTURE_SYSTEM", "Darwin")
            .env("FIXTURE_ARCH", "arm64")
            .env("FIXTURE_LIBC", "glibc 2.39")
            .env("FIXTURE_CHECKSUM", checksum)
            .env("FIXTURE_BINARY_VERSION", "9.8.7")
            .env("FIXTURE_ARCHIVE", self.0.join("release.tar.gz"))
            .env("FIXTURE_REQUESTS", self.0.join("requests"))
            .env_remove("FIXTURE_DOWNLOAD_FAILURE");
        command
    }
    fn requests(&self) -> String {
        fs::read_to_string(self.0.join("requests")).unwrap_or_default()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn supported_platforms_install_from_latest_and_pinned_archives() {
    for (system, arch, target) in [
        ("Darwin", "arm64", "aarch64-apple-darwin"),
        ("Linux", "aarch64", "aarch64-unknown-linux-gnu"),
        ("Linux", "x86_64", "x86_64-unknown-linux-gnu"),
    ] {
        for pinned in [false, true] {
            let fixture = Fixture::new();
            let mut command = fixture.command();
            command
                .env("FIXTURE_SYSTEM", system)
                .env("FIXTURE_ARCH", arch);
            if pinned {
                command.env("NOULI_VERSION", "v9.8.7");
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(
                fs::read(fixture.0.join("destination with spaces/nouli")).unwrap(),
                fs::read(child::executable()).unwrap()
            );
            assert!(fixture.requests().contains(target));
            assert_eq!(fixture.requests().contains("/latest"), !pinned);
        }
    }
}

#[test]
fn invalid_platform_libc_or_version_fails_before_downloading() {
    for (key, value) in [
        ("FIXTURE_ARCH", "armv7l"),
        ("FIXTURE_ARCH", "x86_64"),
        ("FIXTURE_SYSTEM", "Windows_NT"),
        ("NOULI_VERSION", "../../other"),
    ] {
        let fixture = Fixture::new();
        assert!(
            !fixture
                .command()
                .env(key, value)
                .output()
                .unwrap()
                .status
                .success()
        );
        assert_eq!(fixture.requests(), "");
    }
    for libc in ["glibc 2.37", "musl 1.2.5", "", "glibc 2.38"] {
        let fixture = Fixture::new();
        let output = fixture
            .command()
            .env("FIXTURE_SYSTEM", "Linux")
            .env("FIXTURE_LIBC", libc)
            .output()
            .unwrap();
        assert_eq!(output.status.success(), libc == "glibc 2.38");
        if libc != "glibc 2.38" {
            assert_eq!(fixture.requests(), "");
        }
    }
}

#[test]
fn the_distributed_shell_installer_runs_from_standard_input() {
    let fixture = Fixture::new();
    let prototype = fixture.command();
    let mut command = Command::new("sh");
    for (key, value) in prototype.get_envs() {
        if let Some(value) = value {
            command.env(key, value);
        } else {
            command.env_remove(key);
        }
    }
    let mut child = command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    std::io::Write::write_all(
        &mut child.stdin.take().unwrap(),
        include_bytes!("../../scripts/install.sh"),
    )
    .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(fixture.0.join("destination with spaces/nouli").is_file());
}

#[test]
fn failed_download_checksum_or_executable_preserves_the_previous_install() {
    for (key, value) in [
        ("FIXTURE_DOWNLOAD_FAILURE", "1"),
        (
            "FIXTURE_CHECKSUM",
            "0000000000000000000000000000000000000000000000000000000000000000",
        ),
        ("FIXTURE_BINARY_VERSION", "wrong-version"),
    ] {
        let fixture = Fixture::new();
        let destination = fixture.0.join("destination with spaces");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("nouli"), b"old executable").unwrap();
        let output = fixture.command().env(key, value).output().unwrap();
        assert!(!output.status.success());
        assert_eq!(
            fs::read(destination.join("nouli")).unwrap(),
            b"old executable"
        );
        assert_eq!(fs::read_dir(&destination).unwrap().count(), 1);
        assert!(!fs::read_dir(&fixture.0).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("nouli-install.")
        }));
    }
}
