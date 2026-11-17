---
status: accepted
date: 2026-11-17
---

# Support native Windows applications and the terminal CLI

The distribution deferral below is superseded by
[ADR 0030](0030-distribute-windows-release-archives.md).

## Context

The core crate can compile for Windows, but its Unix-only file modes do not
protect a Windows ledger or credential file. The CLI also relies on Unix process
groups, resource limits, output descriptors, and PTYs. Native support must retain
private persistence, bounded subprocess ownership, cancellation, and terminal
cleanup for standard users and administrators.

## Decision

- Support Windows 11 on ARM64 and x64 with the MSVC toolchains. Keep the core's
  explicit path and injected credential boundaries. The CLI chooses a nonempty
  HOME first, then USERPROFILE on Windows, and puts defaults under .nouli.
- Share protected filesystem operations in core storage::private_file. New
  Windows directories and files receive a protected DACL at creation, granting
  access only to the effective user, SYSTEM, and local Administrators. SQLite's
  rollback journal inherits the directory's restricted ACL. This trusts OS
  administrators, as Unix private modes trust root; it adds no encryption or
  Windows Credential Manager dependency.
- Validate existing Windows objects by handle before reading or opening SQLite:
  trusted owner and allowed principals, inheritable directory ACLs, regular
  single-link files, and no final reparse points. Pin the ledger root against
  rename for the connection lifetime. Check existing locks and sidecars even for
  read-only opens. Refuse unsupported WAL stores before SQLite recovery. Refuse
  unsafe objects with actionable errors; never silently rewrite their ACLs.
- Hold stable sibling lock files while updating config, credentials, and terminal
  preferences. Sync a protected temporary sibling, then publish with MoveFileExW
  and write-through. Replacement failure preserves the old file and removes the
  temporary file. There is no delete-before-rename gap.
- Start Windows subprocesses suspended, assign an owned Job Object, then resume.
  Stop the complete job on timeout, cancellation, output overflow, failure, and
  successful converter exit. Drain output while an owned thread writes input;
  terminate before joining blocked input writers. Parser jobs enforce 512 MiB
  process/job memory and eight seconds of process CPU time. Direct parser-worker
  invocation installs the same resource limits. Office output files are checked
  against their byte limit while running; this polling limit can overshoot
  between checks, unlike Unix RLIMIT_FSIZE. Reject reparse points in that tree.
- Credential commands run executable argv directly; batch-file sources require
  an explicitly chosen interpreter. On Windows retain PATH, HOME, SystemRoot,
  USERPROFILE, LOCALAPPDATA, TEMP, and TMP for native program startup; credentials
  and arbitrary environment variables are excluded. Windows itself may inject
  architecture variables. Preserve the ten-second and 64-KiB limits.
- Own and cancel synchronous Windows output writers, including when their
  futures are dropped, so pipe backpressure cannot detach work. Use native
  console Unicode writes and exact byte writes for redirected JSON/JSONL.
- Preserve native text selection from ADR 0021. Save and restore Windows console
  modes and enable VT input so Conhost retains paste wrappers. An owned native
  input reader preserves zero-virtual-key control records that crossterm drops,
  decodes UTF-16/AltGr and mouse/resize/focus records, then normalizes bracketed
  paste before bindings see it. Close its bounded queue, signal stop, and join
  its reader before setup prompts or exit. Treat
  drive, UNC, quoted, and PowerShell file-drop paths as data. Use real
  ConPTY sessions for the existing Rust terminal integration suites.
- Run the Cargo suite and platform-specific Clippy on native Windows ARM64/x64,
  macOS, and Linux in CI. Test a separate consumer against the verified Cargo
  package. Keep two-account ACL acceptance checks in Rust, invoked via Cargo.

This extends the platform-specific process and file-safety details in
[ADR 0003](0003-central-credential-store.md). Its credential format and lookup
precedence remain in force.

## Considered alternatives

- WSL-only support would leave native Windows library consumers and native
  terminal users without the required behavior.
- Compiling out permissions and process limits would report support while
  weakening the existing security and cancellation contracts.
- Windows Credential Manager or DPAPI would change credential persistence and
  migration without protecting the ledger. Restricted filesystem storage fits
  the existing design and supports headless accounts.
- Automatically repairing existing ACLs could change intentional sharing or hide
  prior exposure. Refusal lets the owner review and repair the affected paths.
- A new scripting test runner would duplicate Cargo's product-test entry point.
  Native Rust fixtures and portable-pty supply the missing Windows capabilities.

## Consequences

Windows consumers can use core persistence without the CLI. Standard accounts
need no elevation for normal operation. Existing stores with broad ACLs require
owner review and repair before use. ACL-capable local storage with trusted
ancestor directories is required; network shares, FAT filesystems, GNU targets,
and WSL compatibility are outside this Windows support scope. Converter Job
Objects constrain resources and lifetime; they are not a filesystem or network
sandbox. Clipboard integration remains the terminal's responsibility and needs
an interactive Windows Terminal acceptance check in addition to ConPTY tests.

This change adds source-build support and validation. Windows release archives,
installers, package-manager distribution, and website changes are deferred.
