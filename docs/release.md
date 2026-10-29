# Install Nouli

## Install a release archive

Choose an archive matching your operating system and processor from [GitHub Releases](https://github.com/standing-desks/nouli/releases):

- `nouli-VERSION-aarch64-apple-darwin.tar.gz` for Apple silicon Macs.
- `nouli-VERSION-aarch64-unknown-linux-gnu.tar.gz` for ARM64 GNU/Linux.

Download its matching `.sha256` file. Verify the checksum from the download directory:

```sh
# macOS:
shasum -a 256 --check nouli-VERSION-aarch64-apple-darwin.tar.gz.sha256
# Linux:
sha256sum --check nouli-VERSION-aarch64-unknown-linux-gnu.tar.gz.sha256
```

Replace `VERSION` with the downloaded version. Extract the archive and place `nouli` in a directory on your PATH. For example, on macOS:

```sh
tar -xzf nouli-VERSION-aarch64-apple-darwin.tar.gz
mkdir -p "$HOME/.local/bin"
cp nouli "$HOME/.local/bin/nouli"
nouli --version
```

If your shell cannot find `nouli`, add `$HOME/.local/bin` to your PATH or choose a directory already on it.

Archives contain the executable, the [MIT license](../LICENSE), and a README. They are unsigned and unnotarized. The Linux download targets Ubuntu 24.04 or a compatible GNU/Linux environment; compatibility with older glibc versions is not established.

## Build from source

With the repository checked out and Rustup installed, run from its root:

```sh
cargo install --path nouli-cli --locked
nouli --version
```

The repository selects its Rust toolchain automatically. Building requires the native C toolchain and platform SDK. The installed executable does not need a Rust or Python runtime.

## Configure and update

Follow the [configuration guide](configuration.md) to select a provider, model, and credential variable. Nouli does not include a model or provider account.

To update, replace the installed executable with the new version. Configuration and saved histories remain in their configured locations.

## Remove Nouli

Remove the executable you installed. If you installed with Cargo, run:

```sh
cargo uninstall nouli-cli
```

Saved data in `~/.nouli` or a custom store is separate from the executable. Keep it to preserve your configuration and decision histories.
