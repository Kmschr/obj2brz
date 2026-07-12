# Building obj2brz for Multiple Platforms

This project includes scripts to build obj2brz for both Linux and Windows.

## Quick Start

### First Time Setup

Run the setup script to install cross-compilation dependencies:

```bash
./setup-cross-compile.sh
```

This will:
- Add the Windows (x86_64-pc-windows-gnu) Rust target
- Add the Linux (x86_64-unknown-linux-gnu) Rust target
- Install MinGW-w64 (Windows cross-compiler) for your Linux distribution

### Building

Once setup is complete, build for all platforms:

```bash
./build.sh
```

The compiled binaries will be placed in the `dist/` directory:
- `dist/obj2brz-linux-x86_64` - Linux executable
- `dist/obj2brz-windows-x86_64.exe` - Windows executable (with icon embedded)

## Manual Building

You can also build for specific platforms manually:

### Linux
```bash
cargo build --release --target x86_64-unknown-linux-gnu
```

### Windows
```bash
cargo build --release --target x86_64-pc-windows-gnu
```

## Notes

- The Windows executable will have the obj2brz icon embedded automatically via the `build.rs` script
- Cross-compilation requires MinGW-w64 to be installed on your system
- The build script uses the GNU ABI for Windows compatibility
