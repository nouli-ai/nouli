# Repository Guidelines

## Project Structure & Module Organization

- Put any new Rust crates at the workspace root when they need a separate package.
- **Modules:** Use file-named modules (`foo.rs` next to a `foo/` directory for submodules), never `mod.rs`.
- **Errors:** Define typed errors with `thiserror`, one error enum per module or subsystem rather than one global enum.
- **Logging:** Emit diagnostics with `tracing` macros and spans, never `println!` or `eprintln!`. Only emit events; never install a subscriber. Never log secrets, file contents, or terminal output.
- Add any new dependencies with `cargo add <dependency>`.
  - Do not directly edit cargo.toml to add dependencies.
  - Let cargo pick the version unless a certain version is strictly necessary.

## Architecture Decision Records

- This repository records architectural decisions in [docs/adr/](docs/adr/). Read relevant ADRs before changing architecture or established design choices.
- Record significant architectural decisions in `docs/adr/NNNN-short-title.md`, using the next sequential four-digit number. Include the context, decision, considered alternatives, and consequences; follow the existing ADR format with `status` and `date` frontmatter.
- When replacing an accepted decision, add a new ADR and link the records to show which decision supersedes the earlier one.

## Build, Test, and Development Commands

No build or development commands are runnable in the current checkout. Once `Cargo.toml` and crate sources exist, run these from the workspace root:

- `cargo build --workspace`: compile the workspace.
- `cargo fmt --all`: apply formatting before committing; use `cargo fmt --all --check` for a read-only check.
- `cargo clippy --workspace --all-targets -- -D warnings`: lint code and treat warnings as errors.
- `cargo test --workspace`: run unit, integration, and documentation tests.

Before committing Rust changes, format the code, then run Clippy and the workspace tests. All checks must pass.

Document additional prerequisites and commands alongside the code that introduces them.

## Coding Style & Naming Conventions

- **Rust unit tests:** Unit tests must be inline in a `#[cfg(test)] mod tests` block in the same source file as the code they test.

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

