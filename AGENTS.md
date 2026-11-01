# Repository Guidelines

## Project Structure & Module Organization

- Put any new Rust crates at the workspace root when they need a separate package.
- **Modules:** Use file-named modules (`foo.rs` next to a `foo/` directory for submodules), never `mod.rs`.
- **Errors:** Define typed errors with `thiserror`, one error enum per module or subsystem rather than one global enum.
- **Logging:** Emit diagnostics with `tracing` macros and spans, never `println!` or `eprintln!`. Only emit events; never install a subscriber. Never log secrets, file contents, or terminal output.
- Add any new dependencies with `cargo add <dependency>`.
  - Do not directly edit cargo.toml to add dependencies.
  - Let cargo pick the version unless a certain version is strictly necessary.

## Public copy

Use direct, concrete descriptions in README and website prose. Avoid marketing
slogans built around "X keeps the Y" or paired model/harness taglines.

## Architecture Decision Records

- This repository records architectural decisions in [docs/adr/](docs/adr/). Read relevant ADRs before changing architecture or established design choices.
- Record significant architectural decisions in `docs/adr/NNNN-short-title.md`, using the next sequential four-digit number. Include the context, decision, considered alternatives, and consequences; follow the existing ADR format with `status` and `date` frontmatter.
- When replacing an accepted decision, add a new ADR and link the records to show which decision supersedes the earlier one.

## Build, Test, and Development Commands

Run these commands from the workspace root:

- `cargo build --workspace`: compile the workspace.
- `cargo fmt --all`: apply formatting before committing; use `cargo fmt --all --check` for a read-only check.
- `cargo clippy --workspace --all-targets -- -D warnings`: lint code and treat warnings as errors.
- `cargo test --workspace`: run unit, integration, and documentation tests.

Before committing Rust changes, format the code, then run Clippy and the workspace tests. All checks must pass.

Document additional prerequisites and commands alongside the code that introduces them.

## Testing

- Write tests for Rust crates in Rust. Keep unit tests inline in a `#[cfg(test)] mod tests` block and integration tests in the owning crate’s `tests/` directory, with shared helpers under `tests/support/`.
- Run automated Rust product tests through `cargo test --workspace`. Use platform gating where necessary.
- Reuse the repository’s existing language and tooling for the subsystem being changed. Do not introduce another language, interpreter, or separate test runner merely for implementation convenience. Keep Rust product test logic and harnesses in Rust, executed through Cargo. When a test requires an external tool, document the capability the existing toolchain cannot provide, the added prerequisites, and how the Cargo test invokes it. An interpreter remains a dependency even when preinstalled or used only with its standard library.

## Review

After changing Rust code, have the Rust reviewer agent (`rust-reviewer` in Claude Code, `rust_reviewer` in Codex) review the change, and address its findings before committing.

## Commit & Pull Request Guidelines

- **Branches:** `main` accepts changes only through pull requests, and force pushes are blocked. Do your work on a local branch. Never push, open a pull request, or merge without the user's explicit approval.
- **Commits:** Use `Imperative command`. Write the subject as an
  instruction to edit the repository. A complete subject names both the codebase artifact and the
  edit applied to it, using a repository-edit verb such as `add`, `move`, `split`, `extract`,
  `replace`, `remove`, or `rename`. Keep the message as a short subject line. You can add at most one or
  two body lines if absolutely necessary.
  - **Examples of good commit messages:**
    - `Add pagination middleware to collection endpoints`
    - `Split customer addresses into normalized tables`
    - `Extract checkout form into reusable component`
  - **Reasoning:** States the repository mutation directly, so the commit history reads as a
    sequence of concrete operations that transformed the project.

## Changelog and Releases

- Record notable user-facing changes in `CHANGELOG.md` under `## [Unreleased]`, using the applicable `Added`, `Changed`, `Fixed`, or `Removed` subsection. Describe the resulting behavior for users.
- Only record changes that affect users' experience of the product: using the `nouli` CLI or `nouli-core` library, or installing, updating, or uninstalling them, including the commands and URLs they use to do so. Leave out other changes, such as to the website, documentation, or CI. When one of those accompanies a product change, record only the product change.
- When preparing a release, move its finished entries into one dated section, `## [MAJOR.MINOR.PATCH] - YYYY-MM-DD`, and leave `[Unreleased]` above it. Preserve earlier release entries.
- Match the release section and `vMAJOR.MINOR.PATCH` tag to the workspace version and lockfile. Release notes are extracted from that section; missing, empty, or duplicate sections fail the release.
- Follow [the release workflow](.github/workflows/release.yml) for version-tag builds and draft verification. Correct a published release with a new patch version; preserve published assets and existing release tags.
