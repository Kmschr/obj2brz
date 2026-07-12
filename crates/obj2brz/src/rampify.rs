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

struct Rampifier {
    size: (usize, usize, usize),
    /// RGB values are stored as packed RGB + 1. Zero is air, which keeps the
    /// dense grid at four bytes per voxel and still represents black.
    grid: Vec<u32>,
    occupied_by_ramps: HashSet<usize>,
    origin: Vox,
}

impl Rampifier {
    fn from_octree(octree: &VoxelTree<Vector4<u8>>) -> ConversionResult<Option<Self>> {
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

        if count == 0 {
            return Ok(None);
        }

        let dimension = |low: isize, high: isize| {
            usize::try_from(high - low + 1).map_err(|_| {
                ConversionError::RampifyGridTooLarge("model bounds are not representable".into())
            })
        };
        // Keep a one-voxel air border so flood fill can distinguish exterior
        // air from the enclosed space inside a watertight mesh.
        let origin = Vox(
            min.0.checked_sub(1).ok_or_else(|| ConversionError::RampifyGridTooLarge("model bounds underflowed".into()))?,
            min.1.checked_sub(1).ok_or_else(|| ConversionError::RampifyGridTooLarge("model bounds underflowed".into()))?,
            min.2.checked_sub(1).ok_or_else(|| ConversionError::RampifyGridTooLarge("model bounds underflowed".into()))?,
        );
        let size = (
            dimension(origin.0, max.0.checked_add(1).ok_or_else(|| ConversionError::RampifyGridTooLarge("model bounds overflowed".into()))?)?,
            dimension(origin.1, max.1.checked_add(1).ok_or_else(|| ConversionError::RampifyGridTooLarge("model bounds overflowed".into()))?)?,
            dimension(origin.2, max.2.checked_add(1).ok_or_else(|| ConversionError::RampifyGridTooLarge("model bounds overflowed".into()))?)?,
        );
        let cells = size
            .0
            .checked_mul(size.1)
            .and_then(|value| value.checked_mul(size.2))
            .ok_or_else(|| {
                ConversionError::RampifyGridTooLarge("model bounds overflowed".into())
            })?;
        if cells > MAX_GRID_VOXELS {
            return Err(ConversionError::RampifyGridTooLarge(format!(
                "{} voxels (limit is {}); reduce the model scale",
                cells, MAX_GRID_VOXELS
            )));
        }

        let mut rampifier = Self {
            size,
            grid: vec![0; cells],
            occupied_by_ramps: HashSet::new(),
            origin,
        };
        octree.for_each_leaf(|position, color| {
            let position = octree_to_brickadia_grid(position);
            let local = Vox(
                position.0 - origin.0,
                position.1 - origin.1,
                position.2 - origin.2,
            );
            let index = rampifier
                .index(local)
                .expect("octree leaf must be in its bounds");
            rampifier.grid[index] = pack_color(*color);
        });
        rampifier.fill_enclosed_air();
        Ok(Some(rampifier))
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

    fn fill_gaps(&mut self, opts: &ConvertOptions, output: &mut Vec<Brick>) {
        for index in &self.occupied_by_ramps {
            self.grid[*index] = 0;
        }
        for z in 0..self.size.2 {
            for y in 0..self.size.1 {
                for x in 0..self.size.0 {
                    let position = Vox(x as isize, y as isize, z as isize);
                    let color = self.voxel(position);
                    if color == 0 {
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
                        if self.voxel(position + offset) != color {
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
    let Some(mut rampifier) = Rampifier::from_octree(octree)? else {
        return Ok(());
    };
    opts.logger.log(format!(
        "Rampifying {} voxel cells directly from the octree...",
        rampifier.grid.len()
    ));
    rampifier.generate_ramps(true, opts, &mut save.bricks);
    rampifier.generate_ramps(false, opts, &mut save.bricks);
    rampifier.fill_gaps(opts, &mut save.bricks);
    opts.logger
        .log(format!("Rampify generated {} bricks", save.bricks.len()));
    Ok(())
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
        collision: Default::default(),
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
        assert!(save
            .bricks
            .iter()
            .any(|brick| brick.asset.asset().as_ref() == "PB_DefaultRamp"));
        assert!(save.bricks.len() < 6);
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
