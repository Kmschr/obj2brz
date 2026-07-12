#!/bin/bash

# Build script for obj2brz - builds for both Linux and Windows

set -e  # Exit on error

echo "==================================="
echo "Building obj2brz for all platforms"
echo "==================================="

# Create output directory
mkdir -p dist

echo ""
echo "Building for Linux (x86_64)..."
cargo build --release --target x86_64-unknown-linux-gnu
cp target/x86_64-unknown-linux-gnu/release/obj2brz dist/obj2brz-linux-x86_64

echo ""
echo "Building for Windows (x86_64)..."
cargo build --release --target x86_64-pc-windows-gnu
cp target/x86_64-pc-windows-gnu/release/obj2brz.exe dist/obj2brz-windows-x86_64.exe

echo ""
echo "==================================="
echo "Build complete! Binaries are in dist/"
echo "==================================="
ls -lh dist/
