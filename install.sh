#!/bin/sh
# Build and install Hardpoint system-wide.
#
# On Arch, `makepkg -si` is the better route: it produces a real package that
# pacman can remove cleanly. This script exists for everything else.
set -eu

PREFIX="${PREFIX:-/usr/local}"

if ! command -v cargo >/dev/null 2>&1; then
    echo "cargo not found: install a Rust toolchain first (https://rustup.rs)" >&2
    exit 1
fi

echo "Building Hardpoint (release)..."
make build

echo
echo "Running self-tests..."
./target/release/hardpoint --audit

echo
if [ "$(id -u)" -eq 0 ]; then
    make PREFIX="$PREFIX" install
else
    echo "Installing to $PREFIX requires root."
    sudo make PREFIX="$PREFIX" install
fi
