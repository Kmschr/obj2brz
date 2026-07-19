//! Vendored and BRDB-native rampifier.
//!
//! The slope-selection algorithm is adapted from Wrapperup/rampifier.  The
//! original command-line tool first wrote a BRS containing one 1x1 plate per
//! voxel and then read that save back into a grid.  This version reads the
//! converter octree directly and emits `brdb::Brick`s, so that transient save
//! (and its potentially millions of `Brick` allocations) does not exist.

use std::collections::{HashMap, HashSet, VecDeque};

use brdb::{Brick, BrickSize, BrickType, Color, Direction, Position, Rotation};
use cgmath::Vector4;

use crate::{
    octree::VoxelTree, ConversionError, ConversionResult, ConvertOptions, Material, SaveData,
};

const RAMP_MAX_RUN: usize = 4;
const RAMP_MAX_RISE: usize = 12;
const MAX_GRID_VOXELS: usize = 64 * 1024 * 1024;
/// Neighbouring voxels each chunk loads for context. A ramp reaches at most
/// `RAMP_MAX_RISE` cells up and `RAMP_MAX_RUN` along, so this covers every
/// lookup slope fitting makes across a chunk boundary.
const CHUNK_HALO: isize = RAMP_MAX_RISE as isize;
const EXTERIOR_AIR: u32 = u32::MAX;

#[derive(Clone, Copy)]
struct Vox(isize, isize, isize);

impl std::ops::Add for Vox {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self(self.0 + rhs.0, self.1 + rhs.1, self.2 + rhs.2)
    }
}

impl std::ops::Sub for Vox {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self(self.0 - rhs.0, self.1 - rhs.1, self.2 - rhs.2)
    }
}

impl std::ops::Mul<isize> for Vox {
    type Output = Self;
    fn mul(self, rhs: isize) -> Self {
        Self(self.0 * rhs, self.1 * rhs, self.2 * rhs)
    }
}

impl Vox {
    fn forward(rotation: Rotation) -> Self {
        match rotation {
            Rotation::Deg0 => Self(-1, 0, 0),
            Rotation::Deg90 => Self(0, -1, 0),
            Rotation::Deg180 => Self(1, 0, 0),
            Rotation::Deg270 => Self(0, 1, 0),
        }
    }
}

fn next_rotation(rotation: Rotation) -> Rotation {
    match rotation {
        Rotation::Deg0 => Rotation::Deg90,
        Rotation::Deg90 => Rotation::Deg180,
        Rotation::Deg180 => Rotation::Deg270,
        Rotation::Deg270 => Rotation::Deg0,
    }
}

/// `Direction::ZNegative` mirrors a brick across the world X axis, so a
/// ceiling brick's stored rotation is the reflection of its voxel-space
/// rotation. This is the corner-brick generalization of the Deg0/Deg180 swap
/// `create_ramp` applies to plain ramps.
fn mirrored_rotation(rotation: Rotation) -> Rotation {
    match rotation {
        Rotation::Deg0 => Rotation::Deg90,
        Rotation::Deg90 => Rotation::Deg0,
        Rotation::Deg180 => Rotation::Deg270,
        Rotation::Deg270 => Rotation::Deg180,
    }
}

struct Rampifier {
    size: (usize, usize, usize),
    /// RGB values are stored as packed RGB + 1. Zero is air, which keeps the
    /// dense grid at four bytes per voxel and still represents black.
    grid: Vec<u32>,
    occupied_by_ramps: HashSet<usize>,
    origin: Vox,
    /// Inclusive local bounds of the cells this instance may emit bricks for.
    /// Everything outside is halo or air border.
    core: (Vox, Vox),
}

impl Rampifier {
    /// Whole-octree constructor, kept for tests that build a grid directly.
    /// Conversions go through [`rampify_chunked`], which picks the region.
    #[cfg(test)]
    fn from_octree(octree: &VoxelTree<Vector4<u8>>) -> ConversionResult<Option<Self>> {
        let Some((min, max)) = octree_bounds(octree) else {
            return Ok(None);
        };
        Self::from_octree_region(octree, min, max, 0).map(Some)
    }

    /// Builds a dense grid covering `core_min..=core_max`, surrounded by `halo`
    /// cells of neighbouring voxels and a one-cell air border.
    ///
    /// The halo is read-only context: it feeds slope decisions and the flood
    /// fill, but [`Self::in_core`] keeps every brick anchored inside the core,
    /// so the chunk that owns those cells is the one that emits them. The air
    /// border lets the flood fill tell exterior air from the enclosed space
    /// inside a watertight mesh.
    fn from_octree_region(
        octree: &VoxelTree<Vector4<u8>>,
        core_min: Vox,
        core_max: Vox,
        halo: isize,
    ) -> ConversionResult<Self> {
        let overflowed =
            || ConversionError::RampifyGridTooLarge("model bounds overflowed".into());
        let pad = halo + 1;
        let origin = Vox(
            core_min.0.checked_sub(pad).ok_or_else(overflowed)?,
            core_min.1.checked_sub(pad).ok_or_else(overflowed)?,
            core_min.2.checked_sub(pad).ok_or_else(overflowed)?,
        );
        let far = Vox(
            core_max.0.checked_add(pad).ok_or_else(overflowed)?,
            core_max.1.checked_add(pad).ok_or_else(overflowed)?,
            core_max.2.checked_add(pad).ok_or_else(overflowed)?,
        );

        let dimension = |low: isize, high: isize| {
            usize::try_from(high - low + 1).map_err(|_| {
                ConversionError::RampifyGridTooLarge("model bounds are not representable".into())
            })
        };
        let size = (
            dimension(origin.0, far.0)?,
            dimension(origin.1, far.1)?,
            dimension(origin.2, far.2)?,
        );
        let cells = size
            .0
            .checked_mul(size.1)
            .and_then(|value| value.checked_mul(size.2))
            .ok_or_else(overflowed)?;
        if cells > MAX_GRID_VOXELS {
            return Err(ConversionError::RampifyGridTooLarge(format!(
                "{} voxels (limit is {}); reduce the model scale",
                cells, MAX_GRID_VOXELS
            )));
        }

        // Without a halo there is no neighbouring chunk to keep out of, so the
        // whole grid (air border included) is fair game for brick generation.
        let core = if halo == 0 {
            (
                Vox(0, 0, 0),
                Vox(size.0 as isize - 1, size.1 as isize - 1, size.2 as isize - 1),
            )
        } else {
            (
                Vox(core_min.0 - origin.0, core_min.1 - origin.1, core_min.2 - origin.2),
                Vox(core_max.0 - origin.0, core_max.1 - origin.1, core_max.2 - origin.2),
            )
        };
        let mut rampifier = Self {
            size,
            grid: vec![0; cells],
            occupied_by_ramps: HashSet::new(),
            origin,
            core,
        };
        octree.for_each_leaf(|position, color| {
            let position = octree_to_brickadia_grid(position);
            let local = Vox(
                position.0 - origin.0,
                position.1 - origin.1,
                position.2 - origin.2,
            );
            // Chunked runs see the whole octree, most of which lies outside
            // this chunk's grid.
            if let Some(index) = rampifier.index(local) {
                rampifier.grid[index] = pack_color(*color);
            }
        });
        rampifier.fill_enclosed_air();
        Ok(rampifier)
    }

    fn index(&self, position: Vox) -> Option<usize> {
        if position.0 < 0
            || position.1 < 0
            || position.2 < 0
            || position.0 >= self.size.0 as isize
            || position.1 >= self.size.1 as isize
            || position.2 >= self.size.2 as isize
        {
            return None;
        }
        Some(
            position.0 as usize
                + position.1 as usize * self.size.0
                + position.2 as usize * self.size.0 * self.size.1,
        )
    }

    fn voxel(&self, position: Vox) -> u32 {
        self.index(position).map_or(0, |index| self.grid[index])
    }

    fn exists(&self, position: Vox) -> bool {
        self.voxel(position) != 0
    }

    /// Whether every cell a ramp or corner would consume lies in this grid's
    /// core. A chunk must not emit bricks reaching into its halo: those cells
    /// belong to a neighbouring chunk, and two grids covering the same cells
    /// would overlap in the world. Rejected fits fall through to `fill_gaps`,
    /// which emits plain bricks, so seams stay filled but blockier.
    ///
    /// `position` is the anchor `create_ramp`/`create_corner` derive, which for
    /// ceiling bricks sits `rise - 1` cells below the fitted position.
    fn footprint_in_core(&self, position: Vox, rise: usize, floor: bool, spans: &[(Vox, usize)]) -> bool {
        let anchor = if floor {
            position
        } else {
            position - Vox(0, 0, rise as isize - 1)
        };
        let mut corner = anchor + Vox(0, 0, rise as isize - 1);
        for (axis, length) in spans {
            corner = corner + *axis * (*length as isize - 1);
        }
        self.in_core(anchor) && self.in_core(corner)
    }

    /// Whether a cell may anchor or be consumed by brick generation. Halo and
    /// border cells never can: they exist only as context for the core.
    fn in_core(&self, position: Vox) -> bool {
        let (low, high) = self.core;
        position.0 >= low.0
            && position.1 >= low.1
            && position.2 >= low.2
            && position.0 <= high.0
            && position.1 <= high.1
            && position.2 <= high.2
    }

    fn is_ramp(&self, position: Vox) -> bool {
        self.index(position)
            .is_some_and(|index| self.occupied_by_ramps.contains(&index))
    }

    fn slope_height(&self, position: Vox, floor: bool) -> i32 {
        if !self.exists(position) {
            return i32::MIN;
        }
        let up = if floor { Vox(0, 0, 1) } else { Vox(0, 0, -1) };
        for step in 1..32 {
            if !self.exists(position + up * step) {
                return step as i32;
            }
        }
        31
    }

    fn best_rotation(&self, position: Vox, floor: bool) -> Option<Rotation> {
        let up = if floor { Vox(0, 0, 1) } else { Vox(0, 0, -1) };
        if self.exists(position + up) {
            return None;
        }

        let candidates = [
            (Vox(-1, 0, 0), Vox(1, 0, 0), Rotation::Deg0),
            (Vox(1, 0, 0), Vox(-1, 0, 0), Rotation::Deg180),
            (Vox(0, -1, 0), Vox(0, 1, 0), Rotation::Deg90),
            (Vox(0, 1, 0), Vox(0, -1, 0), Rotation::Deg270),
        ];
        candidates
            .into_iter()
            .filter(|(_, back, _)| !self.exists(position + *back))
            .map(|(forward, _, rotation)| (self.slope_height(position + forward, floor), rotation))
            .max_by_key(|(height, _)| *height)
            .and_then(|(height, rotation)| (height > 0).then_some(rotation))
    }

    fn fit_ramp(&self, position: Vox, rotation: Rotation, floor: bool) -> Option<(usize, usize)> {
        let forward = Vox::forward(rotation);
        let up = if floor { Vox(0, 0, 1) } else { Vox(0, 0, -1) };
        let mut run = 0isize;
        for _ in 0..RAMP_MAX_RUN - 1 {
            if !self.exists(position + up + forward * run)
                && self.exists(position + forward * (run + 1))
                && !self.is_ramp(position + forward * (run + 1))
            {
                run += 1;
            } else {
                break;
            }
        }
        if run == 0 {
            return None;
        }

        let mut rise = 0isize;
        for _ in 1..RAMP_MAX_RISE {
            let tip = position + up * rise + forward * run;
            if self.exists(tip) && !self.is_ramp(tip) {
                rise += 1;
            } else {
                break;
            }
        }
        let mut add_one = 0;
        for step in 1..RAMP_MAX_RUN {
            let beyond_tip = position + up * rise + forward * (run + step as isize);
            if !self.exists(beyond_tip) && !self.is_ramp(beyond_tip) {
                add_one = 1;
            } else {
                add_one = 0;
                break;
            }
        }
        rise += add_one;
        (rise > 1).then_some((run as usize + 1, rise as usize - 1))
    }

    /// Fits a corner ramp whose two high walls face `rotation` and
    /// `rotation + 90`, with `position` as the low outer corner. Returns the
    /// runs along each wall axis and the rise. Corner ramps only fit where a
    /// straight ramp fits along both perpendicular edges and the footprint
    /// between them is clear, so straight edges never match.
    ///
    /// An outer corner (convex contour turn) has open cells behind both wall
    /// axes and rising terrain only past the far diagonal. An inner corner
    /// (concave turn) sits where the edge wraps the other way: the cells
    /// behind it are still part of the edge, only the diagonal between them is
    /// open, and the terrain rises along the whole far row and column.
    fn fit_corner(
        &self,
        position: Vox,
        rotation: Rotation,
        floor: bool,
        inner: bool,
    ) -> Option<(usize, usize, usize)> {
        let up = if floor { Vox(0, 0, 1) } else { Vox(0, 0, -1) };
        let forward_a = Vox::forward(rotation);
        let forward_b = Vox::forward(next_rotation(rotation));
        if self.exists(position + up) {
            return None;
        }
        let corner_shaped = if inner {
            self.exists(position - forward_a)
                && self.exists(position - forward_b)
                && !self.exists(position - forward_a - forward_b)
        } else {
            !self.exists(position - forward_a) && !self.exists(position - forward_b)
        };
        if !corner_shaped {
            return None;
        }
        let (run_a, rise_a) = self.fit_ramp(position, rotation, floor)?;
        let (run_b, rise_b) = self.fit_ramp(position, next_rotation(rotation), floor)?;
        // The two edge fits rarely agree exactly on rough terrain; the lower
        // rise still produces a corner that meets both neighbouring slopes.
        let rise = rise_a.min(rise_b);
        // An outer corner surface is the intersection of the two straight
        // ramps, so it only reaches full height at the far diagonal cell; an
        // inner corner is their union and is full height along the whole
        // far row and column. Those full-height cells may hold the rising
        // terrain (like a straight fit's last column); everywhere else the
        // footprint must be flat with air above.
        let clear = |cells_a: usize, cells_b: usize| {
            for i in 0..cells_a as isize {
                for j in 0..cells_b as isize {
                    let cell = position + forward_a * i + forward_b * j;
                    if !self.exists(cell) || self.is_ramp(cell) {
                        return false;
                    }
                    let on_far_a = i == cells_a as isize - 1;
                    let on_far_b = j == cells_b as isize - 1;
                    let full_height = if inner {
                        on_far_a || on_far_b
                    } else {
                        on_far_a && on_far_b
                    };
                    if !full_height && self.exists(cell + up) {
                        return false;
                    }
                }
            }
            true
        };
        // The full-run footprint is often obstructed on rough terrain, so fall
        // back to the largest clear rectangle that still fits a corner (2x2).
        let mut best: Option<(usize, usize)> = None;
        for cells_a in 2..=run_a {
            for cells_b in 2..=run_b {
                if best.is_none_or(|(a, b)| cells_a * cells_b > a * b) && clear(cells_a, cells_b) {
                    best = Some((cells_a, cells_b));
                }
            }
        }
        best.map(|(cells_a, cells_b)| (cells_a, cells_b, rise))
    }

    fn create_corner(
        &mut self,
        position: Vox,
        (run_a, run_b, rise): (usize, usize, usize),
        inner: bool,
        rotation: Rotation,
        floor: bool,
        opts: &ConvertOptions,
    ) -> Brick {
        let position = if floor {
            position
        } else {
            position - Vox(0, 0, rise as isize - 1)
        };
        let forward_a = Vox::forward(rotation);
        let forward_b = Vox::forward(next_rotation(rotation));

        let mut colors = HashMap::<u32, usize>::new();
        for i in 0..run_a as isize {
            for j in 0..run_b as isize {
                for z in 0..rise as isize {
                    let voxel = position + forward_a * i + forward_b * j + Vox(0, 0, 1) * z;
                    if let Some(index) = self.index(voxel) {
                        self.occupied_by_ramps.insert(index);
                        let color = self.grid[index];
                        if color != 0 {
                            *colors.entry(color).or_default() += 1;
                        }
                    }
                }
            }
        }
        let color = colors
            .into_iter()
            .max_by_key(|(_, count)| *count)
            .map(|(color, _)| color)
            .expect("corner ramp has occupied voxels");

        let global = position + self.origin;
        let far = global + forward_a * (run_a as isize - 1) + forward_b * (run_b as isize - 1);
        let min_x = global.0.min(far.0);
        let min_y = global.1.min(far.1);
        let (x_cells, y_cells) = if forward_a.0 != 0 {
            (run_a, run_b)
        } else {
            (run_b, run_a)
        };
        // Local X follows the first wall axis on floors; the ZNegative mirror
        // swaps the wall axes, so ceiling corners store swapped extents.
        let size = if floor {
            BrickSize::new((run_a * 5) as u16, (run_b * 5) as u16, (rise * 2) as u16)
        } else {
            BrickSize::new((run_b * 5) as u16, (run_a * 5) as u16, (rise * 2) as u16)
        };
        brick(
            if inner {
                "PB_DefaultRampInnerCorner"
            } else {
                "PB_DefaultRampCorner"
            },
            size,
            Position {
                x: (min_x * 10) as i32 + x_cells as i32 * 5,
                y: (min_y * 10) as i32 + y_cells as i32 * 5,
                z: global.2 as i32 * 4 + size.z as i32,
            },
            unpack_color(color),
            if floor {
                Direction::ZPositive
            } else {
                Direction::ZNegative
            },
            if floor { rotation } else { mirrored_rotation(rotation) },
            opts,
        )
    }

    /// Runs the full pass over this grid's core: corners, ramps, then plain
    /// bricks for whatever is left.
    fn generate(&mut self, opts: &ConvertOptions, output: &mut Vec<Brick>) {
        // Corners run first: straight ramps along the edges next to a convex
        // corner would otherwise consume the corner's footprint cells.
        if opts.rampify_corners {
            self.generate_corners(true, opts, output);
        }
        self.generate_ramps(true, opts, output);
        // Terrain mode only smooths upward-facing surfaces; undersides are left
        // for fill_gaps, which emits plain upright bricks.
        if !opts.rampify_terrain {
            if opts.rampify_corners {
                self.generate_corners(false, opts, output);
            }
            self.generate_ramps(false, opts, output);
        }
        self.fill_gaps(opts, output);
    }

    fn generate_corners(&mut self, floor: bool, opts: &ConvertOptions, output: &mut Vec<Brick>) {
        const ROTATIONS: [Rotation; 4] = [
            Rotation::Deg0,
            Rotation::Deg90,
            Rotation::Deg180,
            Rotation::Deg270,
        ];
        for scan_z in 0..self.size.2 as isize {
            let z = if floor {
                scan_z
            } else {
                self.size.2 as isize - 1 - scan_z
            };
            for y in 0..self.size.1 as isize {
                for x in 0..self.size.0 as isize {
                    let position = Vox(x, y, z);
                    if !self.exists(position) || self.is_ramp(position) {
                        continue;
                    }
                    'placed: for rotation in ROTATIONS {
                        for inner in [false, true] {
                            if let Some(fit) = self.fit_corner(position, rotation, floor, inner)
                            {
                                let (run_a, run_b, rise) = fit;
                                let spans = [
                                    (Vox::forward(rotation), run_a),
                                    (Vox::forward(next_rotation(rotation)), run_b),
                                ];
                                if !self.footprint_in_core(position, rise, floor, &spans) {
                                    continue;
                                }
                                output.push(self.create_corner(
                                    position, fit, inner, rotation, floor, opts,
                                ));
                                break 'placed;
                            }
                        }
                    }
                }
            }
        }
    }

    fn create_ramp(
        &mut self,
        position: Vox,
        run: usize,
        rise: usize,
        rotation: Rotation,
        floor: bool,
        opts: &ConvertOptions,
    ) -> Brick {
        let position = if floor {
            position
        } else {
            position - Vox(0, 0, rise as isize - 1)
        };
        let global = position + self.origin;
        let size = BrickSize::new((run * 5) as u16, 5, (rise * 2) as u16);
        let mut rotation = rotation;
        let mut output_position = Position {
            x: global.0 as i32 * 10,
            y: global.1 as i32 * 10,
            z: global.2 as i32 * 4,
        };
        match rotation {
            Rotation::Deg0 => {
                output_position.x += 10 - size.x as i32;
                output_position.y += size.y as i32;
            }
            Rotation::Deg90 => {
                output_position.x += size.y as i32;
                output_position.y += 10 - size.x as i32;
            }
            Rotation::Deg180 => {
                output_position.x += size.x as i32;
                output_position.y += size.y as i32;
            }
            Rotation::Deg270 => {
                output_position.x += size.y as i32;
                output_position.y += size.x as i32;
            }
        }
        output_position.z += size.z as i32;

        let mut colors = HashMap::<u32, usize>::new();
        for x in 0..run as isize {
            for z in 0..rise as isize {
                let voxel = position + Vox::forward(rotation) * x + Vox(0, 0, 1) * z;
                if let Some(index) = self.index(voxel) {
                    self.occupied_by_ramps.insert(index);
                    let color = self.grid[index];
                    if color != 0 {
                        *colors.entry(color).or_default() += 1;
                    }
                }
            }
        }
        let color = colors
            .into_iter()
            .max_by_key(|(_, count)| *count)
            .map(|(color, _)| color)
            .expect("ramp has occupied voxels");
        if !floor {
            rotation = match rotation {
                Rotation::Deg0 => Rotation::Deg180,
                Rotation::Deg180 => Rotation::Deg0,
                other => other,
            };
        }
        brick(
            if run < 2 {
                "PB_DefaultWedge"
            } else {
                "PB_DefaultRamp"
            },
            size,
            output_position,
            unpack_color(color),
            if floor {
                Direction::ZPositive
            } else {
                Direction::ZNegative
            },
            rotation,
            opts,
        )
    }

    fn generate_ramps(&mut self, floor: bool, opts: &ConvertOptions, output: &mut Vec<Brick>) {
        for scan_z in 0..self.size.2 as isize {
            let z = if floor {
                scan_z
            } else {
                self.size.2 as isize - 1 - scan_z
            };
            for y in 0..self.size.1 as isize {
                for x in 0..self.size.0 as isize {
                    let position = Vox(x, y, z);
                    if self.exists(position) && !self.is_ramp(position) {
                        if let Some(rotation) = self.best_rotation(position, floor) {
                            if let Some((run, rise)) = self.fit_ramp(position, rotation, floor) {
                                let spans = [(Vox::forward(rotation), run)];
                                if self.footprint_in_core(position, rise, floor, &spans) {
                                    output.push(
                                        self.create_ramp(position, run, rise, rotation, floor, opts),
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    fn fill_gaps(&mut self, opts: &ConvertOptions, output: &mut Vec<Brick>) {
        for index in &self.occupied_by_ramps {
            self.grid[*index] = 0;
        }
        for z in 0..self.size.2 {
            for y in 0..self.size.1 {
                for x in 0..self.size.0 {
                    let position = Vox(x as isize, y as isize, z as isize);
                    let color = self.voxel(position);
                    if color == 0 || !self.in_core(position) {
                        continue;
                    }
                    let h = self.grow(position, Vox(0, 0, 1), 64, color, (1, 1, 1));
                    let w = self.grow(position, Vox(1, 0, 0), 64, color, (1, 1, h));
                    let l = self.grow(position, Vox(0, 1, 0), 64, color, (w, 1, h));
                    for dx in 0..w {
                        for dy in 0..l {
                            for dz in 0..h {
                                if let Some(index) = self
                                    .index(position + Vox(dx as isize, dy as isize, dz as isize))
                                {
                                    self.grid[index] = 0;
                                }
                            }
                        }
                    }
                    let global = position + self.origin;
                    let size = BrickSize::new((w * 5) as u16, (l * 5) as u16, (h * 2) as u16);
                    output.push(brick(
                        "PB_DefaultBrick",
                        size,
                        Position {
                            x: global.0 as i32 * 10 + size.x as i32,
                            y: global.1 as i32 * 10 + size.y as i32,
                            z: global.2 as i32 * 4 + size.z as i32,
                        },
                        unpack_color(color),
                        Direction::ZPositive,
                        Rotation::Deg0,
                        opts,
                    ));
                }
            }
        }
    }

    fn grow(
        &self,
        position: Vox,
        axis: Vox,
        limit: usize,
        color: u32,
        dimensions: (usize, usize, usize),
    ) -> usize {
        let mut length = 1;
        while length < limit {
            let mut matches = true;
            for x in 0..dimensions.0 {
                for y in 0..dimensions.1 {
                    for z in 0..dimensions.2 {
                        let offset =
                            Vox(x as isize, y as isize, z as isize) + axis * length as isize;
                        let candidate = position + offset;
                        if !self.in_core(candidate) || self.voxel(candidate) != color {
                            matches = false;
                        }
                    }
                }
            }
            if !matches {
                break;
            }
            length += 1;
        }
        length
    }

    /// Rampifier replaces rectangular regions with sloped bricks, so it needs
    /// a volume rather than a one-voxel shell. The OBJ voxelizer records only
    /// intersected surface cells; fill air that is not reachable from the
    /// padded grid boundary to make watertight imports solid. Open meshes are
    /// left unchanged because their air remains connected to the exterior.
    fn fill_enclosed_air(&mut self) {
        let mut exterior = VecDeque::new();
        for z in 0..self.size.2 as isize {
            for y in 0..self.size.1 as isize {
                for x in 0..self.size.0 as isize {
                    if x != 0
                        && y != 0
                        && z != 0
                        && x != self.size.0 as isize - 1
                        && y != self.size.1 as isize - 1
                        && z != self.size.2 as isize - 1
                    {
                        continue;
                    }
                    let position = Vox(x, y, z);
                    let index = self.index(position).unwrap();
                    if self.grid[index] == 0 {
                        self.grid[index] = EXTERIOR_AIR;
                        exterior.push_back(position);
                    }
                }
            }
        }

        while let Some(position) = exterior.pop_front() {
            for offset in [
                Vox(1, 0, 0), Vox(-1, 0, 0), Vox(0, 1, 0),
                Vox(0, -1, 0), Vox(0, 0, 1), Vox(0, 0, -1),
            ] {
                if let Some(index) = self.index(position + offset) {
                    if self.grid[index] == 0 {
                        self.grid[index] = EXTERIOR_AIR;
                        exterior.push_back(position + offset);
                    }
                }
            }
        }

        let interior_color = self
            .grid
            .iter()
            .copied()
            .filter(|color| *color != 0 && *color != EXTERIOR_AIR)
            .fold(HashMap::<u32, usize>::new(), |mut counts, color| {
                *counts.entry(color).or_default() += 1;
                counts
            })
            .into_iter()
            .max_by_key(|(_, count)| *count)
            .map(|(color, _)| color);

        for color in &mut self.grid {
            if *color == EXTERIOR_AIR {
                *color = 0;
            } else if *color == 0 {
                *color = interior_color.unwrap_or(0);
            }
        }
    }
}

pub fn rampify(
    octree: &VoxelTree<Vector4<u8>>,
    save: &mut SaveData,
    opts: &ConvertOptions,
) -> ConversionResult<()> {
    // Callers that want one grid per chunk use `rampify_chunked` directly;
    // here the chunks are concatenated, which is geometrically identical
    // because brick positions are absolute.
    for chunk in rampify_chunked(octree, opts)? {
        save.bricks.extend(chunk);
    }
    Ok(())
}

/// Rampifies an octree into one brick list per chunk.
///
/// The rampifier needs a dense voxel grid, so a model whose bounding box
/// exceeds [`MAX_GRID_VOXELS`] used to be rejected outright. Such a model is
/// instead split into spatial chunks that are rampified independently, each
/// small enough to fit the budget. Callers turn each chunk into its own frozen
/// grid. Models that already fit produce a single chunk.
pub fn rampify_chunked(
    octree: &VoxelTree<Vector4<u8>>,
    opts: &ConvertOptions,
) -> ConversionResult<Vec<Vec<Brick>>> {
    rampify_chunked_with_budget(octree, opts, MAX_GRID_VOXELS)
}

/// [`rampify_chunked`] with an explicit per-chunk voxel budget, so tests can
/// force chunking on a model small enough to assert over.
fn rampify_chunked_with_budget(
    octree: &VoxelTree<Vector4<u8>>,
    opts: &ConvertOptions,
    budget: usize,
) -> ConversionResult<Vec<Vec<Brick>>> {
    let Some((min, max)) = octree_bounds(octree) else {
        return Ok(Vec::new());
    };

    let counts = chunk_counts((min, max), budget);
    let total: usize = counts.0 * counts.1 * counts.2;
    if total == 1 {
        let mut bricks = Vec::new();
        let mut rampifier = Rampifier::from_octree_region(octree, min, max, 0)?;
        opts.logger.log(format!(
            "Rampifying {} voxel cells directly from the octree...",
            rampifier.grid.len()
        ));
        rampifier.generate(opts, &mut bricks);
        opts.logger
            .log(format!("Rampify generated {} bricks", bricks.len()));
        return Ok(vec![bricks]);
    }

    opts.logger.log(format!(
        "Model is too large for a single rampify grid; splitting into {} chunks ({}x{}x{})",
        total, counts.0, counts.1, counts.2
    ));

    let extent =
        |low: isize, high: isize, count: usize| ((high - low + 1) as usize).div_ceil(count) as isize;
    let extents = (
        extent(min.0, max.0, counts.0),
        extent(min.1, max.1, counts.1),
        extent(min.2, max.2, counts.2),
    );

    let mut chunks = Vec::new();
    for iz in 0..counts.2 {
        for iy in 0..counts.1 {
            for ix in 0..counts.0 {
                let core_min = Vox(
                    min.0 + ix as isize * extents.0,
                    min.1 + iy as isize * extents.1,
                    min.2 + iz as isize * extents.2,
                );
                let core_max = Vox(
                    (core_min.0 + extents.0 - 1).min(max.0),
                    (core_min.1 + extents.1 - 1).min(max.1),
                    (core_min.2 + extents.2 - 1).min(max.2),
                );

                let mut rampifier =
                    Rampifier::from_octree_region(octree, core_min, core_max, CHUNK_HALO)?;
                let mut bricks = Vec::new();
                rampifier.generate(opts, &mut bricks);
                if bricks.is_empty() {
                    continue;
                }
                opts.logger.log(format!(
                    "  Chunk {} of {} generated {} bricks",
                    chunks.len() + 1,
                    total,
                    bricks.len()
                ));
                chunks.push(bricks);
            }
        }
    }

    Ok(chunks)
}

/// Splits the model's bounding box until one chunk's dense grid fits the voxel
/// budget, always halving the longest axis so chunks stay roughly cubic.
fn chunk_counts(bounds: (Vox, Vox), budget: usize) -> (usize, usize, usize) {
    let (min, max) = bounds;
    let dims = [
        (max.0 - min.0 + 1).max(1) as usize,
        (max.1 - min.1 + 1).max(1) as usize,
        (max.2 - min.2 + 1).max(1) as usize,
    ];
    let mut counts = [1usize, 1, 1];

    loop {
        // Each chunk carries a halo and an air border on both sides.
        let padding = 2 * (CHUNK_HALO as usize + 1);
        let cells = (0..3)
            .map(|axis| dims[axis].div_ceil(counts[axis]) + padding)
            .try_fold(1usize, |cells, extent| cells.checked_mul(extent));
        match cells {
            Some(cells) if cells <= budget => break,
            _ => {}
        }

        let longest = (0..3)
            .max_by_key(|&axis| dims[axis].div_ceil(counts[axis]))
            .expect("three axes");
        // A chunk cannot shrink below the padding it carries, so give up rather
        // than loop forever on a model that is hopeless in every direction.
        if dims[longest].div_ceil(counts[longest]) <= 1 {
            break;
        }
        counts[longest] += 1;
    }

    (counts[0], counts[1], counts[2])
}

/// Absolute bounds of the octree's occupied cells, in Brickadia grid axes.
fn octree_bounds(octree: &VoxelTree<Vector4<u8>>) -> Option<(Vox, Vox)> {
    let mut min = Vox(isize::MAX, isize::MAX, isize::MAX);
    let mut max = Vox(isize::MIN, isize::MIN, isize::MIN);
    let mut count = 0usize;

    octree.for_each_leaf(|position, _| {
        let position = octree_to_brickadia_grid(position);
        min.0 = min.0.min(position.0);
        min.1 = min.1.min(position.1);
        min.2 = min.2.min(position.2);
        max.0 = max.0.max(position.0);
        max.1 = max.1.max(position.1);
        max.2 = max.2.max(position.2);
        count += 1;
    });

    (count > 0).then_some((min, max))
}

/// The voxelizer retains OBJ coordinates, but Brickadia's default bricks use
/// Z as up while OBJ's vertical axis is Y. This is the same X/Z/Y mapping
/// used by `simplify::create_brick`.
fn octree_to_brickadia_grid(position: cgmath::Vector3<isize>) -> Vox {
    Vox(position.x, position.z, position.y)
}

fn pack_color(color: Vector4<u8>) -> u32 {
    ((u32::from(color.x) << 16) | (u32::from(color.y) << 8) | u32::from(color.z)) + 1
}

fn unpack_color(color: u32) -> Color {
    let color = color - 1;
    Color::new((color >> 16) as u8, (color >> 8) as u8, color as u8)
}

fn brick(
    asset: &str,
    size: BrickSize,
    position: Position,
    color: Color,
    direction: Direction,
    rotation: Rotation,
    opts: &ConvertOptions,
) -> Brick {
    Brick {
        id: None,
        asset: BrickType::from((asset.to_owned(), size)),
        owner_index: None,
        original_owner_index: None,
        position,
        rotation,
        direction,
        collision: brdb::Collision {
            player: opts.player_collision,
            physics: opts.physics_collision,
            ..Default::default()
        },
        visible: true,
        color,
        material: match opts.material {
            Material::Plastic => "BMC_Plastic".into(),
            Material::Glass => "BMC_Glass".into(),
            Material::Glow => "BMC_Glow".into(),
            Material::Metallic => "BMC_Metallic".into(),
            Material::Hologram => "BMC_Hologram".into(),
            Material::Ghost => "BMC_Ghost".into(),
        },
        material_intensity: opts.material_intensity as u8,
        components: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::octree::TreeBody;

    #[test]
    fn direct_rampify_emits_ramps_without_plate_bricks() {
        let mut tree = VoxelTree::new();
        for x in 0..3 {
            *tree.get_mut_or_create(cgmath::Vector3::new(x, 0, 0)) =
                TreeBody::Leaf(Vector4::new(1, 2, 3, 255));
        }
        for x in 0..3 {
            *tree.get_mut_or_create(cgmath::Vector3::new(x, 0, 1)) =
                TreeBody::Leaf(Vector4::new(1, 2, 3, 255));
        }
        let mut save = SaveData {
            bricks: Vec::new(),
            author_name: String::new(),
        };
        rampify(&tree, &mut save, &ConvertOptions::default()).unwrap();
        assert!(save.bricks.iter().any(|brick| {
            matches!(
                brick.asset.asset().as_ref(),
                "PB_DefaultRamp" | "PB_DefaultRampCorner"
            )
        }));
        assert!(save.bricks.len() < 6);
    }

    #[test]
    fn places_corner_ramps_on_convex_plateau_corners() {
        // A 6x6 ground plane with a 3x3 plateau in one corner. The plateau's
        // exposed top corner should become a PB_DefaultRampCorner whose two
        // walls face the plateau interior.
        let mut tree = VoxelTree::new();
        for x in 0..6 {
            for depth in 0..6 {
                *tree.get_mut_or_create(cgmath::Vector3::new(x, 0, depth)) =
                    TreeBody::Leaf(Vector4::new(1, 2, 3, 255));
            }
        }
        for x in 0..3 {
            for depth in 0..3 {
                *tree.get_mut_or_create(cgmath::Vector3::new(x, 1, depth)) =
                    TreeBody::Leaf(Vector4::new(1, 2, 3, 255));
            }
        }
        let mut save = SaveData {
            bricks: Vec::new(),
            author_name: String::new(),
        };
        rampify(&tree, &mut save, &ConvertOptions::default()).unwrap();
        let corner = save
            .bricks
            .iter()
            .find(|brick| brick.asset.asset().as_ref() == "PB_DefaultRampCorner")
            .expect("plateau corner should produce a corner ramp");
        assert!(matches!(corner.direction, Direction::ZPositive));
    }

    #[test]
    fn places_inverted_corner_ramps_on_concave_plateau_corners() {
        // A 12x12 ground plane with an L-shaped plateau leaving a lower bay in
        // one quadrant. The world is large enough that the convex bevels at
        // the plateau's outer corners (max run 4) cannot reach the concave
        // turn, which should become a PB_DefaultRampInnerCorner.
        let mut tree = VoxelTree::new();
        for x in 0..12 {
            for depth in 0..12 {
                *tree.get_mut_or_create(cgmath::Vector3::new(x, 0, depth)) =
                    TreeBody::Leaf(Vector4::new(1, 2, 3, 255));
                if x < 6 || depth < 6 {
                    *tree.get_mut_or_create(cgmath::Vector3::new(x, 1, depth)) =
                        TreeBody::Leaf(Vector4::new(1, 2, 3, 255));
                }
            }
        }
        let mut save = SaveData {
            bricks: Vec::new(),
            author_name: String::new(),
        };
        rampify(&tree, &mut save, &ConvertOptions::default()).unwrap();
        let corner = save
            .bricks
            .iter()
            .find(|brick| brick.asset.asset().as_ref() == "PB_DefaultRampInnerCorner")
            .expect("concave plateau corner should produce an inner corner ramp");
        assert!(matches!(corner.direction, Direction::ZPositive));
    }

    #[test]
    fn corner_ramps_can_be_disabled() {
        let mut tree = VoxelTree::new();
        for x in 0..6 {
            for depth in 0..6 {
                *tree.get_mut_or_create(cgmath::Vector3::new(x, 0, depth)) =
                    TreeBody::Leaf(Vector4::new(1, 2, 3, 255));
                if x < 3 && depth < 3 {
                    *tree.get_mut_or_create(cgmath::Vector3::new(x, 1, depth)) =
                        TreeBody::Leaf(Vector4::new(1, 2, 3, 255));
                }
            }
        }
        let mut save = SaveData {
            bricks: Vec::new(),
            author_name: String::new(),
        };
        let opts = ConvertOptions {
            rampify_corners: false,
            ..ConvertOptions::default()
        };
        rampify(&tree, &mut save, &opts).unwrap();
        assert!(!save.bricks.is_empty());
        assert!(!save.bricks.iter().any(|brick| {
            matches!(
                brick.asset.asset().as_ref(),
                "PB_DefaultRampCorner" | "PB_DefaultRampInnerCorner"
            )
        }));
    }

    #[test]
    fn terrain_mode_never_orients_bricks_downward() {
        // A floating 4x4x2 slab has an exposed underside that would normally
        // receive upside-down (ZNegative) ramps.
        let mut tree = VoxelTree::new();
        for x in 0..4 {
            for depth in 0..4 {
                for height in 2..4 {
                    *tree.get_mut_or_create(cgmath::Vector3::new(x, height, depth)) =
                        TreeBody::Leaf(Vector4::new(1, 2, 3, 255));
                }
            }
        }
        let mut save = SaveData {
            bricks: Vec::new(),
            author_name: String::new(),
        };
        let opts = ConvertOptions {
            rampify: true,
            rampify_terrain: true,
            ..ConvertOptions::default()
        };
        rampify(&tree, &mut save, &opts).unwrap();
        assert!(!save.bricks.is_empty());
        assert!(save
            .bricks
            .iter()
            .all(|brick| matches!(brick.direction, Direction::ZPositive)));

        let mut normal = SaveData {
            bricks: Vec::new(),
            author_name: String::new(),
        };
        rampify(&tree, &mut normal, &ConvertOptions::default()).unwrap();
        assert!(normal
            .bricks
            .iter()
            .any(|brick| matches!(brick.direction, Direction::ZNegative)));
    }

    #[test]
    fn uses_the_same_vertical_axis_as_default_bricks() {
        let mut tree = VoxelTree::new();
        *tree.get_mut_or_create(cgmath::Vector3::new(0, 0, 0)) =
            TreeBody::Leaf(Vector4::new(1, 2, 3, 255));
        *tree.get_mut_or_create(cgmath::Vector3::new(1, 5, 2)) =
            TreeBody::Leaf(Vector4::new(1, 2, 3, 255));

        let rampifier = Rampifier::from_octree(&tree).unwrap().unwrap();
        assert_eq!(rampifier.size, (4, 5, 8));
    }

    #[test]
    fn fills_enclosed_shells_before_generating_ramps() {
        let mut tree = VoxelTree::new();
        for x in 0..3 {
            for y in 0..3 {
                for z in 0..3 {
                    if x == 0 || x == 2 || y == 0 || y == 2 || z == 0 || z == 2 {
                        *tree.get_mut_or_create(cgmath::Vector3::new(x, y, z)) =
                            TreeBody::Leaf(Vector4::new(1, 2, 3, 255));
                    }
                }
            }
        }

        let rampifier = Rampifier::from_octree(&tree).unwrap().unwrap();
        assert_ne!(rampifier.voxel(Vox(2, 2, 2)), 0);
    }
}

#[cfg(test)]
mod chunk_tests {
    use super::*;
    use crate::octree::TreeBody;
    use std::collections::HashSet;

    /// A stepped ridge: tall enough to slope, wide enough to split several
    /// ways, and open on all sides so no enclosed air is involved.
    fn stepped_ridge() -> VoxelTree<Vector4<u8>> {
        let mut tree = VoxelTree::new();
        for x in 0..24isize {
            for z in 0..24isize {
                let height = 1 + (x % 6).min(z % 6);
                for y in 0..height {
                    *tree.get_mut_or_create(cgmath::Vector3::new(x, y, z)) =
                        TreeBody::Leaf(Vector4::new(200, 100, 50, 255));
                }
            }
        }
        tree
    }

    /// The voxel box a brick occupies, in Brickadia grid cells. Brick positions
    /// are centres and sizes are half-extents, in the 10x10x4 units per voxel
    /// that `create_ramp` and `fill_gaps` emit.
    fn brick_cells(brick: &Brick) -> Vec<(isize, isize, isize)> {
        let BrickType::Procedural { size, .. } = &brick.asset else {
            panic!("rampify emits procedural bricks, which carry their size");
        };
        // Sizes are in the brick's local axes; a quarter turn swaps X and Y in
        // world space.
        let size = match brick.rotation {
            Rotation::Deg90 | Rotation::Deg270 => BrickSize::new(size.y, size.x, size.z),
            _ => *size,
        };
        let position = brick.position;
        let low = (
            ((position.x - size.x as i32) / 10) as isize,
            ((position.y - size.y as i32) / 10) as isize,
            ((position.z - size.z as i32) / 4) as isize,
        );
        let extent = (
            (size.x as isize * 2) / 10,
            (size.y as isize * 2) / 10,
            (size.z as isize * 2) / 4,
        );
        let mut cells = Vec::new();
        for dx in 0..extent.0 {
            for dy in 0..extent.1 {
                for dz in 0..extent.2 {
                    cells.push((low.0 + dx, low.1 + dy, low.2 + dz));
                }
            }
        }
        cells
    }

    fn solid_cells(tree: &VoxelTree<Vector4<u8>>) -> HashSet<(isize, isize, isize)> {
        let mut cells = HashSet::new();
        tree.for_each_leaf(|position, _| {
            let position = octree_to_brickadia_grid(position);
            cells.insert((position.0, position.1, position.2));
        });
        cells
    }

    #[test]
    fn small_models_stay_in_one_chunk() {
        let tree = stepped_ridge();
        let chunks = rampify_chunked(&tree, &ConvertOptions::default()).unwrap();
        assert_eq!(chunks.len(), 1);
    }

    #[test]
    fn oversized_models_split_into_several_chunks() {
        let tree = stepped_ridge();
        // A budget this small forces a split on a model that would otherwise
        // fit many times over.
        let chunks = rampify_chunked_with_budget(&tree, &ConvertOptions::default(), 40_000).unwrap();
        assert!(chunks.len() > 1, "expected a split, got {} chunk(s)", chunks.len());
        assert!(chunks.iter().all(|chunk| !chunk.is_empty()));
    }

    #[test]
    fn chunk_seams_leave_no_holes_and_no_overlap() {
        let tree = stepped_ridge();
        let opts = ConvertOptions::default();
        let chunks = rampify_chunked_with_budget(&tree, &opts, 40_000).unwrap();
        assert!(chunks.len() > 1);

        let mut covered = HashSet::new();
        for brick in chunks.iter().flatten() {
            for cell in brick_cells(brick) {
                assert!(
                    covered.insert(cell),
                    "cell {cell:?} is covered by two bricks; chunk grids would overlap"
                );
            }
        }

        for cell in solid_cells(&tree) {
            assert!(
                covered.contains(&cell),
                "solid cell {cell:?} has no brick; a chunk seam left a hole"
            );
        }
    }

    #[test]
    fn chunking_covers_the_same_cells_as_a_single_grid() {
        let tree = stepped_ridge();
        let opts = ConvertOptions::default();

        let single: HashSet<_> = rampify_chunked(&tree, &opts)
            .unwrap()
            .iter()
            .flatten()
            .flat_map(brick_cells)
            .collect();
        let chunked: HashSet<_> = rampify_chunked_with_budget(&tree, &opts, 40_000)
            .unwrap()
            .iter()
            .flatten()
            .flat_map(brick_cells)
            .collect();

        // Ramps also claim the air cells they slope through, so the two passes
        // need not agree cell for cell; the solid surface must match exactly.
        let solid = solid_cells(&tree);
        assert_eq!(
            single.intersection(&solid).count(),
            solid.len(),
            "single-grid rampify left solid cells uncovered"
        );
        assert_eq!(
            chunked.intersection(&solid).count(),
            solid.len(),
            "chunked rampify left solid cells uncovered"
        );
    }
}
