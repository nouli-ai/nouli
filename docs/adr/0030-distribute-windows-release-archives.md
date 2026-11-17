---
status: accepted
date: 2026-11-17
---

# Distribute Windows release archives and a PowerShell installer

## Context

[ADR 0028](0028-native-windows-support.md) established native Windows support but
deferred distribution. Requiring Windows users to install a Rust and C/C++
toolchain prevents them from trying the terminal app directly. Release asset
names and installer URLs also need to agree with the website's release sync from
[ADR 0027](0027-move-website-to-own-repository.md).

## Decision

- Add native x64 and ARM64 MSVC release jobs. Build with the static C runtime so
  released executables need no separate Visual C++ runtime installation. Run
  workspace and release terminal tests before packaging.
- Publish `nouli-VERSION-TARGET.zip` and a matching `.sha256` file for
  `x86_64-pc-windows-msvc` and `aarch64-pc-windows-msvc`. Each ZIP has `nouli.exe`,
  LICENSE, and README.md at its root. Preserve the existing Unix asset names.
- Keep the PowerShell installer in `scripts/install.ps1`. The website publishes
  its release-tag copy at `/install.ps1`, alongside the Unix installer. Use
  Windows PowerShell 5.1 or newer and built-in .NET APIs without elevation.
- Select the native OS architecture, resolve the latest stable release or
  `NOULI_VERSION`, and verify the archive checksum and executable version before
  atomic publication. Extract only the root executable. A busy destination or
  failed verification preserves the existing installation.
- Default to `%LOCALAPPDATA%\Programs\Nouli\bin`; `NOULI_INSTALL_DIR` overrides
  it. Report PATH guidance without editing the registry or shell profile. Keep
  configuration and ledgers separate from installation and removal.
- Test the actual installer through Windows Cargo integration tests. PowerShell
  is required because Rust cannot execute the distributed PowerShell product;
  it is not a second test runner. Rust builds local ZIP fixtures and invokes
  the installer with mocked download cmdlets, with no live release dependency.

This supersedes ADR 0028's release archive and installer deferral and extends ADR
0027's release-sync contract to both installer scripts.

## Considered alternatives

- Source-only distribution would keep the toolchain barrier for ordinary users.
- MSI, MSIX, or package-manager publication would add signing, registration, and
  separate release infrastructure. ZIPs fit the existing checksummed assets and
  support installation by standard accounts.
- A dynamically linked C runtime would require an additional runtime prerequisite
  on otherwise supported Windows machines.
- Automatic PATH changes would modify persistent shell state beyond the chosen
  executable path. Explicit guidance follows the Unix installer's behavior.

## Consequences

Windows installation and updates use the same stable-release boundary as Unix.
The website sync must copy both scripts and preserve older release compatibility.
The executable and scripts remain unsigned; these changes do not add Authenticode,
an MSI, or a package-manager listing. Hosted release builds and publishing still
require the normal approved release process.
