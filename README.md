# obj2brs

a5 adaptation of [textured-voxelizer](https://github.com/CheezBarger/textured-voxelizer) by Suficio

![Voxelized plane](banner.png)

![Rampified import](banner2.png)

Generates textured voxel models from OBJ files.

## Building

This checkout follows the current local `brdb` development crate at
`../brdb/crates/brdb`, matching the companion `bls2brz` project. Build the
native application with:

```sh
cargo build --release
```

The converter can also be compiled for `wasm32-unknown-unknown`:

```sh
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown
```

The WebAssembly target is intended for a browser host; native file and folder
pickers are deliberately unavailable there because browsers do not expose a
writable filesystem path.

## Output

Choose **BRZ** for a compact, ready-to-place Brickadia prefab, or **BRDB** for
an editable Brickadia world directory. Generated bundles now include prefab
metadata and use the configured Brickadia owner for their bundle and bricks.


