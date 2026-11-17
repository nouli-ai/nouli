//! Execute the actual PowerShell installer against local release fixtures through Cargo.
#![cfg(windows)]

#[path = "support/harness.rs"]
#[allow(
    dead_code,
    reason = "installer tests use only the isolated home helper"
)]
mod harness;
#[path = "support/loopback.rs"]
#[allow(
    dead_code,
    reason = "the home helper references the provider fixture type"
)]
mod loopback;
#[path = "support/process.rs"]
#[allow(dead_code, reason = "installer tests use only bounded child ownership")]
mod process;

use harness::Home;
use std::{
    fs::{self, File},
    io::{self, Write},
    path::PathBuf,
    process::{Command, Output, Stdio},
    time::Duration,
};
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

const MOCK_DOWNLOADS: &str = r#"
$ErrorActionPreference = 'Stop'
function Invoke-RestMethod {
    param([string]$Uri, [hashtable]$Headers, [int]$TimeoutSec)
    [IO.File]::AppendAllText($env:FIXTURE_REQUESTS, "$Uri`n")
    return @{ tag_name = "v$env:FIXTURE_VERSION" }
}
function Invoke-WebRequest {
    param([switch]$UseBasicParsing, [string]$Uri, [string]$OutFile, [int]$TimeoutSec)
    [IO.File]::AppendAllText($env:FIXTURE_REQUESTS, "$Uri`n")
    if ($env:FIXTURE_FAILURE -eq 'download') { throw 'fixture download failure' }
    if ($Uri.EndsWith('.sha256')) {
        $name = ($Uri -split '/')[-1] -replace '\.sha256$', ''
        $hash = (Get-FileHash -LiteralPath $env:FIXTURE_ARCHIVE -Algorithm SHA256).Hash
        if ($env:FIXTURE_FAILURE -eq 'checksum') { $hash = '0' * 64 }
        [IO.File]::WriteAllText($OutFile, "$hash  $name`n")
    } else {
        [IO.File]::Copy($env:FIXTURE_ARCHIVE, $OutFile)
    }
}
if ($env:FIXTURE_FAILURE -eq 'relative') {
    Set-Location -LiteralPath (Join-Path $env:HOME 'shell location')
    if ([Environment]::CurrentDirectory -eq (Get-Location).ProviderPath) { throw 'fixture needs different shell and process directories' }
    $env:NOULI_INSTALL_DIR = 'relative-bin'
}
if ($env:FIXTURE_FAILURE -eq 'default') {
    $env:NOULI_INSTALL_DIR = $null
    $env:LOCALAPPDATA = Join-Path $env:HOME 'local app data'
}
$ErrorActionPreference = 'Continue'
$originalPreference = $ErrorActionPreference
$originalPath = $env:PATH
$originalUserPath = [Environment]::GetEnvironmentVariable('Path', 'User')
try { . $env:FIXTURE_INSTALLER } catch { [Console]::Error.WriteLine($_); exit 1 }
if ($ErrorActionPreference -ne $originalPreference) { throw 'installer changed caller preferences' }
if ($env:PATH -cne $originalPath) { throw 'installer changed process PATH' }
if ([Environment]::GetEnvironmentVariable('Path', 'User') -cne $originalUserPath) { throw 'installer changed user PATH' }
"#;

struct Fixture {
    home: Home,
    destination: PathBuf,
    version: String,
}

impl Fixture {
    fn new(failure: &str) -> Self {
        let home = Home::create();
        let destination = home.0.join("installation with spaces 中文");
        fs::create_dir(&destination).unwrap();
        let archive = File::create(home.0.join("release.zip")).unwrap();
        let mut zip = ZipWriter::new(archive);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        if failure != "missing-executable" {
            zip.start_file("nouli.exe", options).unwrap();
            let mut executable = File::open(env!("CARGO_BIN_EXE_nouli")).unwrap();
            io::copy(&mut executable, &mut zip).unwrap();
        }
        zip.start_file("LICENSE", options).unwrap();
        zip.write_all(b"fixture license").unwrap();
        // An unrelated archive entry must never escape the private extraction directory.
        zip.start_file("../escaped-file", options).unwrap();
        zip.write_all(b"must not be extracted").unwrap();
        zip.finish().unwrap();
        if matches!(failure, "declared-size" | "oversized") {
            let path = home.0.join("release.zip");
            let mut bytes = fs::read(&path).unwrap();
            let end = bytes.len() - 22;
            assert_eq!(&bytes[end..end + 4], b"PK\x05\x06");
            let central = usize::try_from(u32::from_le_bytes(
                bytes[end + 16..end + 20].try_into().unwrap(),
            ))
            .unwrap();
            assert_eq!(&bytes[central..central + 4], b"PK\x01\x02");
            assert_eq!(&bytes[central + 46..central + 55], b"nouli.exe");
            let payload = bytes[30..central].to_vec();
            let declared = if failure == "oversized" {
                268_435_457_u32
            } else {
                0
            };
            // Keep the stored payload and its compressed size intact while altering both size declarations.
            bytes[22..26].copy_from_slice(&declared.to_le_bytes());
            bytes[central + 24..central + 28].copy_from_slice(&declared.to_le_bytes());
            assert!(bytes[30..central].iter().eq(payload.iter()));
            assert_eq!(&bytes[central + 24..central + 28], &declared.to_le_bytes());
            fs::write(path, bytes).unwrap();
        }
        fs::write(home.0.join("mock-downloads.ps1"), MOCK_DOWNLOADS).unwrap();
        fs::write(
            home.0.join("install.ps1"),
            include_str!("../../scripts/install.ps1"),
        )
        .unwrap();
        fs::write(home.0.join("configuration-sentinel"), b"preserved").unwrap();
        Self {
            home,
            destination,
            version: if failure == "version" {
                "9.8.7".into()
            } else {
                env!("CARGO_PKG_VERSION").into()
            },
        }
    }

    fn run(&self, failure: &str, pinned: bool) -> Output {
        let mut command = Command::new("powershell.exe");
        command
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(self.home.0.join("mock-downloads.ps1"))
            .current_dir(&self.home.0)
            // Let Windows PowerShell build its own module path instead of
            // inheriting PowerShell 7's incompatible modules on hosted runners.
            .env_remove("PSModulePath")
            .env("NOULI_INSTALL_DIR", &self.destination)
            .env(
                "NOULI_VERSION",
                if pinned {
                    format!("v{}", self.version)
                } else {
                    "latest".into()
                },
            )
            .env("HOME", &self.home.0)
            .env("FIXTURE_VERSION", &self.version)
            .env("FIXTURE_FAILURE", failure)
            .env("FIXTURE_ARCHIVE", self.home.0.join("release.zip"))
            .env("FIXTURE_REQUESTS", self.home.0.join("requests"))
            .env("FIXTURE_INSTALLER", self.home.0.join("install.ps1"))
            .env("TEMP", &self.home.0)
            .env("TMP", &self.home.0)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        process::OwnedChild::new(command.spawn().unwrap()).output(Duration::from_secs(60))
    }

    fn requests(&self) -> String {
        fs::read_to_string(self.home.0.join("requests")).unwrap()
    }

    fn assert_preserved(&self) {
        assert_eq!(
            fs::read(self.destination.join("nouli.exe")).unwrap(),
            b"previous executable"
        );
        assert_eq!(fs::read_dir(&self.destination).unwrap().count(), 1);
        assert_eq!(
            fs::read(self.home.0.join("configuration-sentinel")).unwrap(),
            b"preserved"
        );
        assert!(!self.home.0.join("escaped-file").exists());
        assert!(!fs::read_dir(&self.home.0).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("nouli-install-")
        }));
    }
}

#[test]
fn latest_install_uses_a_windows_zip_and_handles_spaces_without_elevation() {
    let fixture = Fixture::new("");
    let output = fixture.run("", false);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        fs::read(fixture.destination.join("nouli.exe"))
            .unwrap()
            .iter()
            .eq(fs::read(env!("CARGO_BIN_EXE_nouli")).unwrap().iter())
    );
    let requests = fixture.requests();
    assert!(requests.contains("/releases/latest"));
    assert!(requests.contains("-pc-windows-msvc.zip"));
    assert!(requests.contains("-pc-windows-msvc.zip.sha256"));
    assert!(!fixture.home.0.join("escaped-file").exists());
    assert_eq!(fs::read_dir(&fixture.destination).unwrap().count(), 1);
}

#[test]
fn a_pinned_install_replaces_the_old_executable_without_latest_discovery() {
    let fixture = Fixture::new("");
    fs::write(
        fixture.destination.join("nouli.exe"),
        b"previous executable",
    )
    .unwrap();
    let output = fixture.run("", true);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!fixture.requests().contains("/releases/latest"));
    assert!(
        fs::read(fixture.destination.join("nouli.exe"))
            .unwrap()
            .iter()
            .eq(fs::read(env!("CARGO_BIN_EXE_nouli")).unwrap().iter())
    );
    assert_eq!(fs::read_dir(&fixture.destination).unwrap().count(), 1);
}

#[test]
fn failed_download_checksum_version_or_archive_preserves_the_installation() {
    for failure in [
        "download",
        "checksum",
        "version",
        "missing-executable",
        "declared-size",
        "oversized",
    ] {
        let fixture = Fixture::new(failure);
        fs::write(
            fixture.destination.join("nouli.exe"),
            b"previous executable",
        )
        .unwrap();
        let output = fixture.run(failure, false);
        assert!(!output.status.success(), "{failure}");
        fixture.assert_preserved();
    }
}

#[test]
fn a_busy_executable_is_not_deleted_during_an_update() {
    use std::os::windows::fs::OpenOptionsExt;
    let fixture = Fixture::new("");
    let destination = fixture.destination.join("nouli.exe");
    fs::write(&destination, b"previous executable").unwrap();
    let locked = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(&destination)
        .unwrap();
    let output = fixture.run("", true);
    assert!(!output.status.success());
    drop(locked);
    fixture.assert_preserved();
}

#[test]
fn a_relative_directory_uses_the_powershell_location() {
    let mut fixture = Fixture::new("");
    let location = fixture.home.0.join("shell location");
    fs::create_dir(&location).unwrap();
    fixture.destination = location.join("relative-bin");
    let output = fixture.run("relative", true);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(fixture.destination.join("nouli.exe").is_file());
    assert!(!fixture.home.0.join("relative-bin").exists());
}

#[test]
fn the_default_install_uses_local_app_data_and_the_native_windows_architecture() {
    let mut fixture = Fixture::new("");
    fixture.destination = fixture.home.0.join("local app data/Programs/Nouli/bin");
    let output = fixture.run("default", true);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(fixture.destination.join("nouli.exe").is_file());
    let architecture = std::env::var("PROCESSOR_ARCHITEW6432")
        .or_else(|_| std::env::var("PROCESSOR_ARCHITECTURE"))
        .unwrap();
    let target = match architecture.to_ascii_lowercase().as_str() {
        "arm64" => "aarch64-pc-windows-msvc",
        "amd64" => "x86_64-pc-windows-msvc",
        other => panic!("unsupported test host architecture: {other}"),
    };
    assert!(fixture.requests().contains(&format!("-{target}.zip")));
}
