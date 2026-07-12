#!/bin/bash

# Build script for obj2brz - builds the CLI and GUI for Linux and Windows

set -e  # Exit on error

echo "==================================="
echo "Building obj2brz for all platforms"
echo "==================================="

# Create output directory
mkdir -p dist

# obj2brz  -> CLI binary (crate: obj2brz-cli)
# obj2brz-gui -> desktop GUI binary (crate: obj2brz-gui)
build_target() {
    local target="$1" suffix="$2" ext="$3"
    echo ""
    echo "Building for ${suffix} (${target})..."
    cargo build --release --target "$target" -p obj2brz-cli -p obj2brz-gui
    cp "target/${target}/release/obj2brz${ext}" "dist/obj2brz-${suffix}${ext}"
    cp "target/${target}/release/obj2brz-gui${ext}" "dist/obj2brz-gui-${suffix}${ext}"
}

build_target x86_64-unknown-linux-gnu linux-x86_64 ""
build_target x86_64-pc-windows-gnu windows-x86_64 ".exe"

echo ""
echo "==================================="
echo "Build complete! Binaries are in dist/"
echo "==================================="
ls -lh dist/
