# Install Nouli

## macOS: install with Homebrew

```sh
brew install nouli-ai/tap/nouli
nouli --version
```

The formula installs the published binary for Apple silicon macOS or ARM64 GNU/Linux. Linux requires system glibc 2.38 or newer; the release is tested on Ubuntu 24.04. Rust is not required for a Homebrew installation.

To update:

```sh
brew update
brew upgrade nouli-ai/tap/nouli
```

To uninstall:

```sh
brew uninstall nouli-ai/tap/nouli
```

Uninstalling keeps your configuration and saved sessions in `~/.nouli` or your custom store.

If you previously installed with Cargo, remove that executable before switching so it does not shadow the Homebrew installation:

```sh
cargo uninstall nouli-cli
brew install nouli-ai/tap/nouli
command -v nouli
nouli --version
```

If you installed an archive manually, remove that copy from your PATH before switching. Follow the [configuration guide](configuration.md) after installation.

## Linux: install a release binary

```sh
curl -fsSL https://nouli.ai/install.sh | sh
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
curl -fsSL https://nouli.ai/install.sh | NOULI_VERSION=0.1.0 sh
curl -fsSL https://nouli.ai/install.sh | NOULI_INSTALL_DIR="$HOME/bin" sh
```

You can download and inspect the script before running it:

```sh
curl -fsSL https://nouli.ai/install.sh -o install.sh
less install.sh
sh install.sh
```

The script's source is [`scripts/install.sh`](../scripts/install.sh). When a stable version is released, nouli.ai publishes the copy from that release, so changes to the script go live with the next release.

## Windows 11: install a release binary

Starting with 0.6.0, ARM64 and x64 ZIP archives and a PowerShell installer are
available. In Windows PowerShell 5.1 or newer, run:

```powershell
irm https://nouli.ai/install.ps1 | iex
```

The installer selects the native Windows architecture, verifies the SHA-256
checksum and executable version, and installs to
`%LOCALAPPDATA%\Programs\Nouli\bin`. It requires no administrator rights or
separate Visual C++ runtime installation. Add that directory to your user PATH
through Windows Settings, or launch directly:

```powershell
& "$env:LOCALAPPDATA\Programs\Nouli\bin\nouli.exe"
```

To select a version or directory, set environment variables before running the
installer:

```powershell
$env:NOULI_VERSION = '0.6.0'
$env:NOULI_INSTALL_DIR = "$env:USERPROFILE\bin"
irm https://nouli.ai/install.ps1 | iex
```

You can inspect the script before running it:

```powershell
Invoke-WebRequest https://nouli.ai/install.ps1 -OutFile install.ps1
Get-Content install.ps1
```

Its source is [`scripts/install.ps1`](../scripts/install.ps1); the website syncs
the copy from each stable release. The installer does not change the registry,
PATH, configuration, or saved history. To update, close Nouli and rerun it. A
failed update preserves the previous executable. To uninstall the default copy:

```powershell
Remove-Item "$env:LOCALAPPDATA\Programs\Nouli\bin\nouli.exe"
```

Remove its user PATH entry if you added one. Saved sessions and credentials stay
under your configured Nouli home. See [Windows development](windows.md) for
building from source and optional document converters.

If another installation comes first on your PATH, remove it or adjust your PATH to select the new executable. Configuration and saved histories are separate from the executable.

## Install a release archive

Choose an archive matching your operating system and processor from [GitHub Releases](https://github.com/nouli-ai/nouli/releases):

- `nouli-VERSION-aarch64-apple-darwin.tar.gz` for Apple silicon Macs.
- `nouli-VERSION-aarch64-unknown-linux-gnu.tar.gz` for ARM64 GNU/Linux.
- `nouli-VERSION-x86_64-unknown-linux-gnu.tar.gz` for x86_64 GNU/Linux, in releases after `0.1.0`.
- `nouli-VERSION-aarch64-pc-windows-msvc.zip` for Windows 11 ARM64, starting with `0.6.0`.
- `nouli-VERSION-x86_64-pc-windows-msvc.zip` for Windows 11 x64, starting with `0.6.0`.

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

On Windows, verify and extract a downloaded ZIP in PowerShell:

```powershell
$archive = 'nouli-VERSION-aarch64-pc-windows-msvc.zip'
$expected = (Get-Content "$archive.sha256").Split(' ')[0]
if ((Get-FileHash $archive -Algorithm SHA256).Hash -ine $expected) { throw 'Checksum mismatch' }
Expand-Archive $archive -DestinationPath nouli-release
.\nouli-release\nouli.exe --version
```

Choose the x64 ZIP instead on an x64 PC. Released Windows binaries use a static
C runtime and need only the supported Windows OS; they are not Authenticode-signed.

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
