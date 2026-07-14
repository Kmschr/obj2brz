# obj2brz

a5 adaptation of [textured-voxelizer](https://github.com/CheezBarger/textured-voxelizer) by Suficio

![Voxelized plane](banner.png)

![Rampified import](banner2.png)

Generates textured voxel models from OBJ, STL, glTF/GLB, and FBX files
(FBX on desktop/CLI only; the browser build cannot run the C-based FBX
parser), plus LDraw models (`.dat`, `.ldr`, `.mpd`).

LDraw part references resolve against the model's own directory and an
installed [LDraw parts library](https://library.ldraw.org/)
(`LDRAWDIR`, `~/.ldraw`, or `/usr/share/ldraw`); colours come from the
library's `LDConfig.ldr` when present, falling back to a built-in table.
LDraw models always convert at true scale — 1 LEGO stud = 1 Brickadia stud
(10 units) — so the scale option does not apply to them.

## Workspace layout

obj2brz is a Cargo workspace split into three crates so the conversion engine is
usable independently of any UI:

- **`crates/obj2brz`** — the UI-agnostic core library. Build a `ConvertOptions`
  and call `obj2brz::convert`.
- **`crates/obj2brz-cli`** — a standalone command-line front-end (`obj2brz`).
- **`crates/obj2brz-gui`** — the eframe desktop application (`obj2brz-gui`),
  which also compiles to `wasm32-unknown-unknown` for a browser host.

The BRDB writer is tracked from upstream
[`brickadia-community/brdb`](https://github.com/brickadia-community/brdb).

## Building

```sh
cargo build --release                 # everything
cargo build --release -p obj2brz-cli  # just the CLI
cargo build --release -p obj2brz-gui  # just the GUI
```

The GUI can also be compiled for `wasm32-unknown-unknown`:

```sh
rustup target add wasm32-unknown-unknown
cargo build --release -p obj2brz-gui --target wasm32-unknown-unknown
```

The WebAssembly target is intended for a browser host; native file and folder
pickers are deliberately unavailable there because browsers do not expose a
writable filesystem path.

## CLI usage

```sh
obj2brz model.obj -o builds -n my_save --scale 2 --simplify
obj2brz model.obj --material hologram --no-player-collision
obj2brz castle.ldr             # LDraw always converts at 1 stud = 1 stud
obj2brz --help
```

Use `--rampify` to generate a slope-focused save with default ramps and
wedges. Rampify runs directly from the converter's voxel octree, rather than
building an intermediate save containing one 1×1 plate per voxel, and works
for both BRZ and BRDB output.

Use `--material` to apply a Brickadia material such as `plastic`, `glass`,
`glow`, `metallic`, `hologram`, or `ghost` to the whole export. Use
`--no-player-collision` and/or `--no-physics-collision` for decorative models
that should not block players or participate in physics/grid collisions.

## Library usage

```rust
use obj2brz::{convert, ConvertOptions};

let opts = ConvertOptions {
    input_file_path: "model.obj".into(),
    output_directory: "builds".into(),
    save_name: "my_save".into(),
    ..ConvertOptions::default()
};
convert(&opts, false)?;
```

## Output

Choose **BRZ** for a compact, ready-to-place Brickadia prefab, or **BRDB** for
an editable Brickadia world directory. Generated bundles include prefab metadata
and use the configured Brickadia owner for their bundle and bricks.
