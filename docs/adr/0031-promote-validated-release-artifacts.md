---
status: accepted
date: 2026-11-17
---

# Promote artifacts from native shipping-configuration validation

## Context

PR, main, and tag workflows compile the same product repeatedly with different
Windows runtime flags and cache keys. Linux ARM64 has no PR gate. Installer
behavior checks for the Unix release live in the downstream website repository.

## Decision

Use one target inventory and reusable native validation workflow for all five
shipping targets. Set explicit targets and Windows static-runtime flags before
cache restoration. Retain Clippy, workspace/example/documentation tests, native
packaged-library consumers, and optimized terminal tests. Validate API docs once
on Linux. Installer-only changes select the owning Cargo tests.

Successful main validation retains archives and checksums for 30 days. Tags
must match the workspace and lockfile versions, a dated nonempty changelog
section, and a commit belonging to main. Promote only complete, unexpired
artifacts from successful CI runs for that exact SHA. Never promote PR runs.
If artifacts are absent, dispatch the same full validation at the approved tag;
do not silently rebuild through a different release configuration.

Package verification compiles and runs an independent consumer against an
extraction of the actual .crate archive. This replaces cargo package's extra
standalone build with the stronger downstream application check on each native
target. Rust Cargo tests own both installers' behavior. POSIX sh, tar, shasum,
Bash, and jq are existing Unix/CI prerequisites: Rust cannot execute the shipped
shell product or the workflow's Bash/jq policy itself. Rust implements tool
stand-ins, archive fixtures, assertions, and test orchestration; no new test
runner or interpreter is introduced.

This extends ADR 0030's validation-before-packaging requirement to trusted main
builds and changes ADR 0027's Unix installer test ownership. Downstream release
notifications run independently so one failure cannot block the other.

## Considered alternatives

Repeating complete tag builds wastes compilation and lets configurations drift.
Promoting PR artifacts would cross the repository's publication trust boundary.
Skipping examples or downstream consumers would reduce meaningful coverage.

## Consequences

Ordinary tags verify and draft existing artifacts. Recovery may still take a
full native validation run. Cache reuse remains subject to GitHub branch scope.
Cargo compilation timing artifacts expose cold/warm performance for subsequent
optimization; slow fixture suites keep their assertions and isolation.
