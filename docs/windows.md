# Native Windows development and validation

Nouli supports source builds for Windows 11 ARM64 and x64 using the MSVC Rust
targets. Both nouli-core and the CLI run natively. The UTM ARM64 VM can also run
x64 executables through Windows emulation; CI uses a native x64 runner for that
target. See the [recorded validation results](windows-validation.md) for checks already
completed and remaining desktop acceptance. Windows release downloads, a Windows installer, package-manager
integration, and website instructions are deferred.

## Build prerequisites

Install Rustup and the repository's pinned Rust toolchain. Install Visual Studio
2022 Build Tools with the Windows SDK and C++ tools for the target architecture.
ARM64 additionally needs LLVM clang-cl on PATH. NASM is recommended for x64
cryptographic assembly builds. CMake is useful for native dependency builds.
See the [AWS-LC Windows requirements](https://aws.github.io/aws-lc-rs/requirements/windows.html)
for the TLS dependency's compiler requirements. Git is needed for source checkouts.
No Python runtime or separate product-test runner is required.

From a PowerShell window in the workspace root:

~~~powershell
rustup show active-toolchain
cargo build --workspace --locked
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked -- --test-threads=4
cargo test -p nouli-core --test package_consumer --locked -- --ignored
~~~

The last command verifies the actual Cargo package, then builds and runs a
separate application against its unpacked contents. It does not publish a crate
or use the CLI as a library. It runs offline after the workspace dependencies
have been fetched. The packaged core includes its own embedding documentation.

For another installed MSVC target, append --target aarch64-pc-windows-msvc or
--target x86_64-pc-windows-msvc to build/test commands. Set CARGO_BUILD_TARGET to
the same target when running package_consumer so its nested Cargo commands
build the same architecture. Use a matching C/C++ toolchain and Windows SDK.

~~~powershell
cargo build --workspace --bins --examples --release --locked
cargo test -p nouli-cli --release --locked --test smoke --test tui_selection --test tui_references --test tui_model_picker --test tui_startup -- --test-threads=4
.\target\release\nouli.exe
~~~

First launch opens the interactive view and shows `/connect` guidance when no
model is configured. Setup begins only when requested.

The terminal suites use portable-pty's native ConPTY backend, including a
cursor-query responder. Subprocess fixtures are Rust executables built by
cargo test --workspace. A focused test invocation that needs the fixture should
first run cargo build -p nouli-cli --example nouli-test-child with the same
profile and target. Avoid simultaneous Cargo invocations using the same target
directory while tests are running.

## Paths and private persistence

The CLI uses a nonempty HOME when set, otherwise USERPROFILE on Windows. Defaults
are .nouli/config.toml, .nouli/auth.toml, .nouli/tui.toml, and the ledger under
that home. Explicit --config and --store paths retain their meaning. Core callers
always supply paths and credential lookup themselves.

Use local storage that supports Windows security descriptors, with trusted
ancestor directories. New private objects grant access to the effective user,
SYSTEM, and local Administrators at creation. Directories propagate only those
permissions to SQLite journals and new children. Administrators are trusted;
plain-text API keys are protected by ACLs, without encryption or a dependency
on Windows Credential Manager. Normal operation works in a standard account.

Existing unsafe roots, files, locks, links, or sidecars are refused without
changing their permissions. Core reports StorageError::Insecure; CLI reports
store_insecure for a ledger or auth_store_insecure for credentials. Refusal
happens before reading contents or SQLite recovery. Read-only opening does not
create files, migrate schemas, or repair permissions. WAL stores are rejected.

Inspect the affected path with icacls before repairing it. Remove untrusted
explicit grants and broad inherited grants; retain only the account, SYSTEM
(S-1-5-18), and Administrators (S-1-5-32-544). Directory grants must propagate to
both files and directories. Check ownership too. For example, in PowerShell:

~~~powershell
icacls "$env:USERPROFILE\.nouli"
icacls "$env:USERPROFILE\.nouli\auth.toml"
~~~

The repair depends on the existing grants. icacls /grant:r only replaces the
named principal's grants; it does not remove other explicit grants. Nouli never
runs a blanket ACL repair on existing data. Follow the error's named path and
inspect the store, locks, and any journal sidecars before retrying.

## Optional document tools

PDF text import uses Poppler's pdftotext.exe on PATH. Legacy .doc and .ppt import
uses LibreOffice's soffice.com/soffice.exe on PATH or under ProgramFiles or
ProgramFiles(x86)/LibreOffice/program. Use tools built for the target Windows
installation; a 64-bit tool can also run under Windows ARM64 emulation.
Modern Office formats, spreadsheets, text, and images retain their existing
Rust-based paths. Nouli does not install converters automatically. Missing
converters produce an actionable import error and retain the unsent draft.
After building the workspace and test fixtures, exercise installed converters
with `cargo test -p nouli-cli --bin nouli --locked -- installed_ --ignored`.
The ordinary suite separately verifies missing-tool errors.

Converters run in owned Job Objects. Timeout, cancellation, output overflow, and
successful launcher exit all clean up descendants. Parser workers receive
512-MiB process/job memory and eight-second per-process CPU limits. Office output
file sizes are monitored while conversion runs; polling can overshoot the byte
limit between checks. This is a resource/lifetime boundary, not a general
filesystem or network sandbox.

Credential command sources use executable argv directly, with no implicit
shell. A .cmd or .bat source must name its interpreter explicitly. The retained
Windows environment is PATH, HOME, SystemRoot, USERPROFILE, LOCALAPPDATA, TEMP,
and TMP; Windows can add architecture variables. Shell/API-key variables are
not forwarded. Command output and errors are never logged or reported.

## Two-account permission acceptance

Use an elevated administrator account and a distinct standard account. Choose a
new shared path whose final name starts with nouli-acl-. Set the same value in
each account's PowerShell session. The fixture refuses an existing path and
creates a public parent as a positive control, so private leaf ACLs are tested
independently of profile-directory protection.

~~~powershell
$env:NOULI_TEST_ACCOUNTS_ROOT = 'C:\Users\Public\nouli-acl-acceptance'
~~~

Run this in the administrator account:

~~~powershell
cargo test -p nouli-cli --test windows_accounts --locked -- administrator_creates_private_artifacts_under_public_parent --ignored --exact
cargo test -p nouli-core --lib --locked -- final_reparse_points_are_refused --ignored
~~~

The reparse test needs symbolic-link privilege or Windows Developer Mode. Then,
in the standard account, run:

~~~powershell
cargo test -p nouli-cli --test windows_accounts --locked -- standard_account_is_denied_private_artifacts --ignored --exact
~~~

The standard-account test proves public-parent writes work while direct OS reads
and writes of the other account's database, credential file, and locks fail with
PermissionDenied. Finish in the administrator account:

~~~powershell
cargo test -p nouli-cli --test windows_accounts --locked -- administrator_cleans_up_account_fixture --ignored --exact
~~~

Also run the full workspace suite in each account. Cross-account fixtures are
ignored in ordinary CI because changing the test's process token cannot replace
an independent Windows login.

## Interactive terminal acceptance

In Windows Terminal, verify ordinary text dragging and Copy, Ctrl-G graph mouse
capture and resizing, Esc/Tab returning to native selection, Ctrl-P hold/resume,
Unicode and AltGr input, multiline paste containing command-like text, file drops
with spaces/Unicode/drive and UNC paths, model picking, Ctrl-C during a provider
request, and restoration after exit. Check the resulting console modes in the
same terminal session. ConPTY verifies protocol and application behavior;
clipboard interaction still belongs to the host terminal.
