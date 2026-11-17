# Validation and releases

`scripts/ci/targets.json` is the shipping target inventory. CI uses the same
explicit target, locked dependencies, toolchain, and Windows static-runtime
flags in PRs, main, and recovery builds. Native validation runs Clippy, standard
workspace/example tests, documentation tests, a downstream consumer of the
actual packaged core, and optimized terminal tests. Linux also builds API docs.
Keep the explicit fixture build before `--all-targets`: examples with embedded
unit tests and the subprocess fixture need different Cargo target forms.

The complete local Rust entry points remain `cargo fmt --all --check`,
`cargo clippy --workspace --all-targets -- -D warnings`, and
`cargo test --workspace`. Run the package consumer explicitly with
`cargo test -p nouli-core --test package_consumer -- --ignored`.

The Unix installer and release-policy regressions also run through Cargo. They
need the existing POSIX shell, tar, shasum, Bash, and jq tooling on Unix. Rust
cannot execute the shipped shell installer or the actual workflow policy
without those interpreters/tools. All fixture generation, GitHub/download
stand-ins, assertions, and test orchestration remain Rust; tests never call
GitHub or publish a release. Windows installer tests use Windows PowerShell.

## Candidate promotion

Successful full main validation retains `candidate-TARGET` artifacts for 30
days. Each holds its archive and checksum. A tag checks version, lockfile,
dated release notes, main ancestry, and successful CI for the exact SHA. It
requires all five unexpired candidates and validates every expected asset and
digest before creating a draft. Published assets and tags are never replaced.

If candidates are missing or expired, the controller dispatches `CI` at that
approved tag with `run_all=true`, waits for the same native checks, and promotes
its artifacts. Recovery API errors fail closed; a failed validation prevents
draft creation. Promotion intentionally refuses PR runs.

## Performance

Configure the native target and runtime flags before restoring the per-target
shipping cache. Cache writes after failures preserve dependencies within the
run's permitted GitHub branch scope. PR caches do not warm main's cache.
Compilation timing reports are retained for seven days. Compare cold and warm
runs and individual Cargo suite durations before changing fixture isolation or
concurrency. The ARM64 installer matrix retains every failure case; its current
host-dependent startup cost warrants measurement rather than dropping coverage.
