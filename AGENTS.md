# Repository Guidelines

## Project Structure & Module Organization

- Put any new Rust crates at the workspace root when they need a separate package.
- **Modules:** Use file-named modules (`foo.rs` next to a `foo/` directory for submodules), never `mod.rs`.
- **Errors:** Define typed errors with `thiserror`, one error enum per module or subsystem rather than one global enum.
- **Logging:** Emit diagnostics with `tracing` macros and spans, never `println!` or `eprintln!`. Only emit events; never install a subscriber. Never log secrets, file contents, or terminal output.
- **Formatting and checks:** Run `make fmt-lib` before committing, then `make check-lib`. It runs the rustfmt check, Clippy with warnings denied, and the workspace tests, and must pass.
- Add any new dependencies with `cargo add <dependency>`.
  - Do not directly edit cargo.toml to add dependencies.
  - Let cargo pick the version unless a certain version is strictly necessary.

## Build, Test, and Development Commands

No build or development commands are runnable in the current checkout. Once `Cargo.toml` and crate sources exist, run these from the crate root:

- `cargo build`: compile the crate.
- `cargo test`: run unit, integration, and documentation tests.
- `cargo fmt --check`: check formatting; use `cargo fmt` to apply changes.
- `cargo clippy --all-targets -- -D warnings`: lint code and treat warnings as errors.

Document additional prerequisites and commands alongside the code that introduces them.

## Coding Style & Naming Conventions

- **Rust unit tests:** Unit tests must be inline in a `#[cfg(test)] mod tests` block in the same source file as the code they test.

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

