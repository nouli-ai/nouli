# Install Nouli

## macOS: install with Homebrew

```sh
brew install standing-desks/tap/nouli
nouli --version
```

The formula installs the published binary for Apple silicon macOS or ARM64 GNU/Linux. Linux requires system glibc 2.38 or newer; the release is tested on Ubuntu 24.04. Rust is not required for a Homebrew installation.

To update:

```sh
brew update
brew upgrade standing-desks/tap/nouli
```

To uninstall:

```sh
brew uninstall standing-desks/tap/nouli
```

Uninstalling keeps your configuration and saved sessions in `~/.nouli` or your custom store.

If you previously installed with Cargo, remove that executable before switching so it does not shadow the Homebrew installation:

```sh
cargo uninstall nouli-cli
brew install standing-desks/tap/nouli
command -v nouli
nouli --version
```

If you installed an archive manually, remove that copy from your PATH before switching. Follow the [configuration guide](configuration.md) after installation.

## Linux: install a release binary

```sh
curl -fsSL https://standing-desks.github.io/nouli/install.sh | sh
```

The script selects the archive for Linux x86_64 or ARM64, verifies its SHA-256 checksum, and installs `nouli` into `~/.local/bin`. It also supports Apple silicon macOS. Linux requires GNU libc 2.38 or newer; musl-based systems such as Alpine need a source build.

Add `~/.local/bin` to your PATH if your shell cannot find the command:

```sh
export PATH="$HOME/.local/bin:$PATH"
nouli --version
```

The installer does not change your shell profile or saved Nouli data. To update, run the installer again. To uninstall its default executable:

```sh
rm "$HOME/.local/bin/nouli"
```

To select a release or another installation directory, set variables for the shell running the installer:

```sh
curl -fsSL https://standing-desks.github.io/nouli/install.sh | NOULI_VERSION=0.1.0 sh
curl -fsSL https://standing-desks.github.io/nouli/install.sh | NOULI_INSTALL_DIR="$HOME/bin" sh
```

You can download and inspect the script before running it:

```sh
curl -fsSL https://standing-desks.github.io/nouli/install.sh -o install.sh
less install.sh
sh install.sh
```

If another installation comes first on your PATH, remove it or adjust your PATH to select the new executable. Configuration and saved histories are separate from the executable.

## Install a release archive

Choose an archive matching your operating system and processor from [GitHub Releases](https://github.com/standing-desks/nouli/releases):

- `nouli-VERSION-aarch64-apple-darwin.tar.gz` for Apple silicon Macs.
- `nouli-VERSION-aarch64-unknown-linux-gnu.tar.gz` for ARM64 GNU/Linux.
- `nouli-VERSION-x86_64-unknown-linux-gnu.tar.gz` for x86_64 GNU/Linux, in releases after `0.1.0`.

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
