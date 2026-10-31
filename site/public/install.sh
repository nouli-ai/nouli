#!/bin/sh
# Install a published Nouli binary. Override NOULI_VERSION or NOULI_INSTALL_DIR as needed.
set -eu

fail() {
  printf 'nouli installer: %s\n' "$*" >&2
  exit 1
}

for tool in curl tar mktemp awk grep; do
  command -v "$tool" >/dev/null 2>&1 || fail "$tool is required"
done
if command -v sha256sum >/dev/null 2>&1; then
  checksum_tool=sha256sum
elif command -v shasum >/dev/null 2>&1; then
  checksum_tool=shasum
else
  fail 'sha256sum or shasum is required'
fi

system=$(uname -s)
arch=$(uname -m)
case "$system:$arch" in
  Darwin:arm64|Darwin:aarch64) target=aarch64-apple-darwin ;;
  Linux:aarch64|Linux:arm64) target=aarch64-unknown-linux-gnu ;;
  Linux:x86_64|Linux:amd64) target=x86_64-unknown-linux-gnu ;;
  *) fail "unsupported platform $system/$arch; see https://github.com/nouli-ai/nouli/releases" ;;
esac
if [ "$system" = Linux ]; then
  libc=$(getconf GNU_LIBC_VERSION 2>/dev/null || true)
  printf '%s\n' "$libc" | awk '
    $1 == "glibc" { split($2, v, "."); if (v[1] > 2 || (v[1] == 2 && v[2] >= 38)) ok = 1 }
    END { exit !ok }
  ' || fail 'GNU/Linux with system glibc 2.38 or newer is required (Ubuntu 24.04 is supported)'
fi

releases=https://github.com/nouli-ai/nouli/releases
version=${NOULI_VERSION:-latest}
if [ "$version" = latest ]; then
  release_url=$(curl -fsSL --proto '=https' --tlsv1.2 --connect-timeout 10 --max-time 30 \
    -o /dev/null -w '%{url_effective}' "$releases/latest") || fail 'could not find the latest release'
  version=${release_url##*/}
fi
version=${version#v}
printf '%s\n' "$version" | grep -Eq '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$' \
  || fail 'NOULI_VERSION must be latest or a stable version such as 0.1.0'

install_dir=${NOULI_INSTALL_DIR:-"${HOME:?Set HOME or NOULI_INSTALL_DIR}/.local/bin"}
installer_tmp=$(mktemp -d "${TMPDIR:-/tmp}/nouli-install.XXXXXX")
staged_binary=
cleanup() {
  rm -rf "$installer_tmp"
  if [ -n "$staged_binary" ]; then rm -f "$staged_binary"; fi
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM

archive=nouli-$version-$target.tar.gz
base=$releases/download/v$version
curl -fsSL --proto '=https' --tlsv1.2 --connect-timeout 10 --max-time 300 \
  "$base/$archive" -o "$installer_tmp/$archive" \
  || fail "no downloadable $target archive for v$version; check $releases"
curl -fsSL --proto '=https' --tlsv1.2 --connect-timeout 10 --max-time 30 \
  "$base/$archive.sha256" -o "$installer_tmp/$archive.sha256" \
  || fail 'could not download the release checksum'
expected=$(awk -v name="$archive" '$2 == name { print $1 }' "$installer_tmp/$archive.sha256")
printf '%s\n' "$expected" | grep -Eq '^[0-9a-f]{64}$' || fail 'invalid release checksum'
if [ "$checksum_tool" = sha256sum ]; then
  actual=$(sha256sum "$installer_tmp/$archive" | awk '{print $1}')
else
  actual=$(shasum -a 256 "$installer_tmp/$archive" | awk '{print $1}')
fi
[ "$actual" = "$expected" ] || fail 'release checksum mismatch; existing installation was not changed'
tar -xzf "$installer_tmp/$archive" -C "$installer_tmp" nouli
if [ ! -f "$installer_tmp/nouli" ] || [ -L "$installer_tmp/nouli" ]; then
  fail 'archive does not contain a regular nouli executable'
fi

[ ! -d "$install_dir/nouli" ] || fail "$install_dir/nouli is a directory"
mkdir -p "$install_dir"
staged_binary=$(mktemp "$install_dir/.nouli.XXXXXX")
cp "$installer_tmp/nouli" "$staged_binary"
chmod 755 "$staged_binary"
reported=$("$staged_binary" --version) || fail 'the downloaded executable cannot run on this system'
[ "$reported" = "nouli $version" ] || fail 'the executable version does not match the release'
mv -f "$staged_binary" "$install_dir/nouli"
staged_binary=
printf 'Installed Nouli %s to %s/nouli\n' "$version" "$install_dir"
case ":${PATH:-}:" in
  *":$install_dir:"*) ;;
  *) printf 'Add %s to your PATH to run nouli.\n' "$install_dir" ;;
esac
