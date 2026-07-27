# obj2brz

a5 adaptation of [textured-voxelizer](https://github.com/CheezBarger/textured-voxelizer) by Suficio

![Voxelized plane](banner.png)

![Rampified import](banner2.png)

Generates textured voxel models from OBJ, STL, glTF/GLB, and FBX files
(FBX on desktop/CLI only; the browser build cannot run the C-based FBX
parser).

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
obj2brz --help
```

Use `--rampify` to generate a slope-focused save with default ramps and
wedges. Rampify runs directly from the converter's voxel octree, rather than
building an intermediate save containing one 1×1 plate per voxel, and works
for both BRZ and BRDB output.

Use `--grid-mesh` for the experimental surface path. Each source face becomes
one or two thin micro-wedges, with one averaged texture/material color per
face. Coplanar wedges share a frozen grid when their in-plane orientation and
integer-local positioning are compatible. Adjacent triangles that form a
convex coplanar quad are fitted against their shared diagonal so all of their
wedges can use one grid. Scale 1 maps one model unit to one Brickadia stud.
Use `--wedge-thickness` to make those wedges thicker than the minimum 0.2
studs; the value is rounded to the nearest 0.2 studs.
This representation preserves floating-point face planes but can still create
up to two dynamic grids per source triangle in the worst case, so start with
small models and inspect rendering, collision, and seams in game.

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
