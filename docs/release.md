# Local installation and release validation

This is an unsigned local validation build. Public publication and distribution are separate owner decisions. The repository has no declared Nouli license or package-license metadata; this procedure selects no license. Review available dependency licenses/notices, final package metadata, repository/support URLs, versioning policy, and release authority before public distribution. No signing/notarization, registry upload, Git tag, GitHub release, or push is performed.

Verified native targets are `aarch64-apple-darwin` and `aarch64-unknown-linux-gnu`. The Linux artifact is built/tested on Debian Bookworm (glibc 2.36); compatible newer GNU/Linux environments still need their own deployment validation. macOS validation uses the local Apple silicon host/SDK; it does not establish a minimum supported macOS version. Windows, x86_64, musl/static Linux, and other targets have no release support claim here.

## Reproduce a local artifact

Use a clean tracked checkout, Rust 1.99.1 with Cargo/rustfmt/Clippy, the native C toolchain/SDK, Git, system TLS roots, and Python 3.11 or later. The first build downloads only locked dependencies; subsequent offline Cargo checks can use the populated cache. Use native builds rather than cross-compiling and claiming runtime validation. From the repository root:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
python3 scripts/validate_docs.py
# OUTPUT must be a new directory; it may live under ignored target/:
python3 scripts/package_release.py --output target/local-release/native
# Replace ARCHIVE with the exact .tar.gz path printed above:
python3 scripts/verify_install.py ARCHIVE.tar.gz
```

The archive includes `bin/nouli`, installation/project/configuration/output/contract documentation, `config.example.toml`, the exact lockfile/toolchain file, `MANIFEST.json`, `THIRD_PARTY_NOTICES.md`, available dependency license/notice files under `licenses/`, and `DISTRIBUTION_STATUS.md`. It includes no model, credential value, home directory, or saved ledger. Development commands in project/API documentation require a separate source checkout; the archive is a native binary distribution, not a buildable source bundle.

The manifest identifies the actual native target, verbose toolchain identity, source commit, any tracked worktree changes, binary SHA-256, and SHA-256 for every staged file except the manifest itself. Release verification should use a clean tracked checkout so `tracked_changes` is empty; unrelated untracked local designs are never packaged. The adjacent `.sha256` file authenticates archive contents only when obtained from a trusted source; it is not a signature.

Archive entries are sorted, have fixed owner/group IDs, normalized permissions, and the source commit's timestamp; gzip embeds that same timestamp and no local filename. Repeating with identical source/documentation, metadata, binary, and timestamp gives identical archive bytes. Locked dependencies do not by themselves guarantee identical binary bytes across different compiler/linker/SDK versions, build paths, environment flags, or targets. Reproduce those inputs, compare the manifest and binary hash first, then compare the archive. Local checksums can also be recomputed with `shasum -a 256 ARCHIVE.tar.gz` on macOS or `sha256sum ARCHIVE.tar.gz` on Linux.

`verify_install.py` requires the matching native host before executing a staged binary. It checks the archive/filename hash, rejects links/traversal/duplicate entries, checks every manifested file and the binary, copies it into a fresh temporary installation, and runs the real CLI walkthrough with an isolated home/config/store and owned loopback service. This includes help/version with no home, configuration loading, durable new/continued turns, completed replay, sourced ephemeral evaluation with unchanged store bytes, and workflow partial/resume. It reports the exact archive/binary checksums and removes its temporary installation. Provider answers are deterministic fixtures.

## Install and remove manually

Obtain and verify an archive for the matching tested native target. Inspect its manifest/status/notices first. Extract into a directory you own, then copy `bin/nouli` into a chosen directory on your PATH, for example a user-owned `~/.local/bin`. The binary has no Python or Rust runtime dependency. SQLite is bundled into the build, and TOON codec/runtime dependencies are linked Rust code; no separate SQLite server or codec installation is required. Native OS libraries and trusted TLS roots remain platform prerequisites. Configure a provider separately using `config.example.toml` and the configuration guide. Local Laya must already be running; no service/model is installed by Nouli.

Uninstall by removing the binary you installed and any extracted artifact directory. Saved data in `~/.nouli` or an explicit store is independent of the executable; retain it unless you explicitly intend to delete history. This procedure never deletes a real user store.
