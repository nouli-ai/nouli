# Windows port validation

Validation performed on 2026-11-17 for the Windows source-support change on
add-windows-support, using the pinned Rust 1.99.1 toolchain. Commands and
prerequisites are in [Windows development and validation](windows.md).

## Completed checks

| Environment | Check | Result |
| --- | --- | --- |
| macOS ARM64 | Workspace build, formatting, Clippy with warnings denied, full Cargo tests, and API documentation | Passed |
| macOS ARM64 | Independent application built against the verified Cargo package of nouli-core | Passed |
| Windows 11 ARM64, standard account | Full workspace unit, integration, and documentation tests | Passed |
| Windows 11 ARM64, standard account | Workspace Clippy with warnings denied; final native input regression tests | Passed |
| Windows 11 ARM64, administrator account | Full workspace unit, integration, and documentation tests | Passed |
| Windows 11 running x64 executables, administrator account | Full workspace tests for x86_64-pc-windows-msvc | Passed through Windows ARM64 emulation |
| Windows 11 ARM64, standard account | Independent application built against the actual verified nouli-core package | Passed |
| Windows x64, administrator account | Independent application built against the actual verified nouli-core package with an explicit cross target | Passed through Windows ARM64 emulation |
| Windows 11 ARM64, standard account | Optimized workspace/example build; release smoke, selection, references, and model-picker suites | Passed |
| Windows 11 ARM64, standard account | 0.6.0 release build with static C runtime; full release workspace suite; executable dependency inspection | Passed; no separate Visual C++ runtime DLL imports |
| Windows x64, standard account | Static-runtime 0.6.0 release build, smoke/startup/selection suites, and executable dependency inspection | Passed through ARM64 emulation; no separate Visual C++ runtime DLL imports |
| Windows ARM64 and x64, standard account | Actual PowerShell installer with local release fixtures: fresh/pinned installs, default/relative paths, native target, caller state, checksum/version/archive failures, bounded extraction, busy destinations, and cleanup | Passed on ARM64 and through x64 emulation |
| macOS ARM64 and Windows 11 ARM64, standard account | First launch enters the TUI without setup prompts; missing-model guidance; explicit setup preserves the draft; credential helpers and providers remain idle at startup | Passed, including the Windows release build |
| Windows 11 ARM64, administrator account | Real Poppler PDF extraction and LibreOffice legacy-document conversion | Passed |
| Windows 11 ARM64, two independent accounts | Administrator creates private artifacts under a public parent; standard account receives OS PermissionDenied for protected reads/writes; administrator removes fixture | Passed |
| Windows 11 ARM64, elevated administrator | File and directory symbolic links are refused without changing original content | Passed |
| Repository | Actionlint and git diff --check | Passed |

The Windows host is an ARM64 UTM VM. These local x64 results establish execution
through Windows emulation; they are not tests on physical x64 hardware. The CI
matrix runs native x64 Windows and native ARM64 Windows runners, alongside Linux
and macOS. Hosted results are recorded with the pull request checks; the table
above records local validation.

The ordinary workspace suite includes hard-link refusal, existing unsafe ACLs
left unchanged, actual live journal/guard ACLs, concurrent ledger creation,
failed replacement preserving the old file, bounded duplex pipes, blocked-input
cancellation, descendant cleanup, direct-worker limits, credential environment
scrubbing, blocked-output future cleanup, and idle/full native-input queue
cleanup. The terminal suites use real ConPTY sessions and check exact console
mode restoration through an isolated launcher. Optional-tool and cross-account
checks are opt-in Cargo tests because they require external capabilities or a
distinct Windows login.

## Remaining release acceptance

Windows Terminal clipboard interaction, ordinary dragging, and physical keyboard
layouts still need the [interactive acceptance check](windows.md#interactive-terminal-acceptance).
Synthetic native key records cover Unicode, surrogate pairs, AltGr, Alt+numpad,
Ctrl-H, and modifier decoding, but do not replace that desktop check.

Windows ZIP release jobs, a PowerShell installer, and website installation
instructions are included. Hosted release builds, publication, and website
deployment remain pending. Package-manager integration remains deferred.
