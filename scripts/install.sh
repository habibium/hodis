#!/bin/sh
# Installs hodis from its GitHub releases: the latest, or the one
# HODIS_VERSION names (such as v0.1.0), into HODIS_INSTALL_DIR, which is
# ~/.local/bin unless set.
#
#   curl -fsSL https://github.com/habibium/hodis/releases/latest/download/hodis-installer.sh | sh

set -eu

repo=habibium/hodis
version=${HODIS_VERSION:-latest}
install_dir=${HODIS_INSTALL_DIR:-$HOME/.local/bin}

fail() {
    echo "hodis installer: $*" >&2
    exit 1
}

case $(uname -s) in
Linux) system=unknown-linux-musl ;;
Darwin) system=apple-darwin ;;
*) fail "there is no hodis build for $(uname -s); install it with: cargo install hodis" ;;
esac

machine=$(uname -m)
# A shell running under Rosetta reports x86_64 on Apple silicon.
if [ "$system" = apple-darwin ] && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || true)" = 1 ]; then
    machine=arm64
fi
case $machine in
x86_64 | amd64) arch=x86_64 ;;
arm64 | aarch64) arch=aarch64 ;;
*) fail "there is no hodis build for $machine; install it with: cargo install hodis" ;;
esac

target=$arch-$system
archive=hodis-$target.tar.gz
if [ "$version" = latest ]; then
    url=https://github.com/$repo/releases/latest/download
else
    url=https://github.com/$repo/releases/download/$version
fi

if command -v curl >/dev/null 2>&1; then
    fetch() { curl --proto '=https' --tlsv1.2 -fsSL "$1" -o "$2"; }
elif command -v wget >/dev/null 2>&1; then
    fetch() { wget -q -O "$2" "$1"; }
else
    fail "downloading hodis needs curl or wget"
fi

if command -v sha256sum >/dev/null 2>&1; then
    check() { sha256sum -c "$1" >/dev/null; }
elif command -v shasum >/dev/null 2>&1; then
    check() { shasum -a 256 -c "$1" >/dev/null; }
else
    fail "checking the download needs sha256sum or shasum"
fi

temp=$(mktemp -d)
trap 'rm -rf "$temp"' EXIT

echo "Downloading hodis for $target"
fetch "$url/$archive" "$temp/$archive" || fail "cannot download $url/$archive"
fetch "$url/$archive.sha256" "$temp/$archive.sha256" || fail "cannot download $url/$archive.sha256"
(cd "$temp" && check "$archive.sha256") || fail "the download does not match its checksum"

tar -xzf "$temp/$archive" -C "$temp"
mkdir -p "$install_dir"
# Moved into place rather than copied over, so a hodis still running keeps its file.
cp "$temp/hodis-$target/hodis" "$install_dir/hodis.new"
chmod 755 "$install_dir/hodis.new"
mv -f "$install_dir/hodis.new" "$install_dir/hodis"

echo "Installed $("$install_dir/hodis" --version) into $install_dir"
case :$PATH: in
*:"$install_dir":*) ;;
*) echo "$install_dir is not on your PATH yet; add it there to run hodis by name." ;;
esac
