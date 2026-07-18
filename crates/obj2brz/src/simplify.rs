use crate::octree::{TreeBody, VoxelTree};
use crate::{BrickType, ConvertOptions, SaveData};

use brdb::{Brick, BrickSize, BrickType as BrdbBrickType, Color, Direction, Position, Rotation};
use cgmath::{Vector3, Vector4};
use std::collections::{HashMap, HashSet, VecDeque};

pub fn simplify_lossy(
    octree: &mut VoxelTree<Vector4<u8>>,
    save_data: &mut SaveData,
    opts: &ConvertOptions,
    max_merge: isize,
) {
    let scales: (isize, isize, isize) = if opts.bricktype == BrickType::Microbricks {
        (opts.brick_scale, opts.brick_scale, opts.brick_scale)
    } else {
        (5, 5, 2)
    };

    let max_scale = isize::max(isize::max(scales.0, scales.1), scales.2);
    let max_merge = max_merge / max_scale;

    loop {
        let mut colors = Vec::<Vector4<u8>>::new();
        let (x, y, z);
        {
            let (location, voxel) = octree.get_any_mut_or_create();

            x = location[0];
            y = location[1];
            z = location[2];

            match voxel {
                TreeBody::Leaf(leaf_color) => {
                    colors.push(*leaf_color);
                }
                _ => break,
            }
        }

        let mut xp = x + 1;
        let mut yp = y + 1;
        let mut zp = z + 1;

        // Expand z direction first due to octree ordering followed by y and x
        // Ensures blocks are simplified in the pattern of Morton coding
        // Saves us having to check in the negative directions
        while zp - z < max_merge {
            let voxel = octree.get_mut_or_create(Vector3::new(x, y, zp));
            match voxel {
                TreeBody::Leaf(leaf_color) => {
                    colors.push(*leaf_color);
                    zp += 1
                }
                _ => break,
            }
        }

        while yp - y < max_merge {
            let mut pass = true;
            for sz in z..zp {
                let voxel = octree.get_mut_or_create(Vector3::new(x, yp, sz));
                match voxel {
                    TreeBody::Leaf(leaf_color) => colors.push(*leaf_color),
                    _ => {
                        pass = false;
                        break;
                    }
                }
            }
            if !pass {
                break;
            }
            yp += 1;
        }

        while xp - x < max_merge {
            let mut pass = true;
            for sy in y..yp {
                for sz in z..zp {
                    let voxel = octree.get_mut_or_create(Vector3::new(xp, sy, sz));
                    match voxel {
                        TreeBody::Leaf(leaf_color) => colors.push(*leaf_color),
                        _ => {
                            pass = false;
                            break;
                        }
                    }
                }
                if !pass {
                    break;
                }
            }
            if !pass {
                break;
            }
            xp += 1;
        }

        // Clear nodes
        // This cant be done during the loops above unless you keep track
        // of which nodes you have already deleted
        for sx in x..xp {
            for sy in y..yp {
                for sz in z..zp {
                    let voxel = octree.get_mut_or_create(Vector3::new(sx, sy, sz));
                    *voxel = TreeBody::Empty;
                }
            }
        }

        let color = rgba_to_brick_color(average_rgba(&colors));

        let width = xp - x;
        let height = yp - y;
        let depth = zp - z;

        save_data.bricks.push(create_brick(
            opts,
            scales,
            (width, depth, height),
            (x, z, y),
            color,
        ));
    }
}

pub fn simplify_lossless(
    octree: &mut VoxelTree<Vector4<u8>>,
    save_data: &mut SaveData,
    opts: &ConvertOptions,
    max_merge: isize,
) {
    let d: isize = 1 << octree.size;
    let len = d + 1;

    let scales: (isize, isize, isize) = if opts.bricktype == BrickType::Microbricks {
        (opts.brick_scale, opts.brick_scale, opts.brick_scale)
    } else {
        (5, 5, 2)
    };

    let max_scale = isize::max(isize::max(scales.0, scales.1), scales.2);
    let max_merge = max_merge / max_scale;

    loop {
        let source_color;
        let color;
        let x;
        let y;
        let z;
        {
            let (location, voxel) = octree.get_any_mut_or_create();

            x = location[0];
            y = location[1];
            z = location[2];

            match voxel {
                TreeBody::Leaf(leaf_color) => {
                    source_color = *leaf_color;
                    color = rgba_to_brick_color(source_color);
                }
                _ => break,
            }
        }

        let mut xp = x + 1;
        let mut yp = y + 1;
        let mut zp = z + 1;

        // Expand z direction first due to octree ordering followed by y
        // Ensures blocks are simplified in the pattern of Morton coding
        while zp < len && (zp - z) < max_merge {
            let voxel = octree.get_mut_or_create(Vector3::new(x, y, zp));
            match voxel {
                TreeBody::Leaf(leaf_color) => {
                    if *leaf_color != source_color {
                        break;
                    }
                    zp += 1;
                }
                _ => break,
            }
        }

        while yp < len && (yp - y) < max_merge {
            let mut pass = true;
            for sz in z..zp {
                let voxel = octree.get_mut_or_create(Vector3::new(x, yp, sz));
                match voxel {
                    TreeBody::Leaf(leaf_color) => {
                        if *leaf_color != source_color {
                            pass = false;
                            break;
                        }
                    }
                    _ => {
                        pass = false;
                        break;
                    }
                }
            }
            if !pass {
                break;
            }
            yp += 1;
        }

        while xp < len && (xp - x) < max_merge {
            let mut pass = true;
            for sy in y..yp {
                for sz in z..zp {
                    let voxel = octree.get_mut_or_create(Vector3::new(xp, sy, sz));
                    match voxel {
                    TreeBody::Leaf(leaf_color) => {
                            if *leaf_color != source_color {
                                pass = false;
                                break;
                            }
                        }
                        _ => {
                            pass = false;
                            break;
                        }
                    }
                }
                if !pass {
                    break;
                }
            }
            if !pass {
                break;
            }
            xp += 1;
        }

        // Clear nodes
        // This cant be done during the loops above unless you keep track
        // of which nodes you have already deleted
        for sx in x..xp {
            for sy in y..yp {
                for sz in z..zp {
                    let voxel = octree.get_mut_or_create(Vector3::new(sx, sy, sz));
                    *voxel = TreeBody::Empty;
                }
            }
        }

        let width = xp - x;
        let height = yp - y;
        let depth = zp - z;

        save_data.bricks.push(create_brick(
            opts,
            scales,
            (width, depth, height),
            (x, z, y),
            color,
        ));
    }
}

/// Racegen-style "squarish" voxel packer.
///
/// The Morton-order greedy passes (`simplify_lossy`/`simplify_lossless`) seed
/// each brick from a region corner and grow z→y→x to maximum extent. On curved
/// or diagonal shells that yields long single-voxel strips and visible seams.
/// This pass instead seeds from the *deepest interior* of each equal-key region
/// (a boundary-distance flood) and grows the currently shortest side first, so
/// bricks come out blocky and inscribed — far fewer seams on the visible shell.
/// Ported from racegen's `rect_merge`, generalized from 2D rects to 3D boxes.
pub fn simplify_squarish(
    octree: &VoxelTree<Vector4<u8>>,
    save_data: &mut SaveData,
    opts: &ConvertOptions,
    max_merge: isize,
    lossy: bool,
) {
    let scales: (isize, isize, isize) = if opts.bricktype == BrickType::Microbricks {
        (opts.brick_scale, opts.brick_scale, opts.brick_scale)
    } else {
        (5, 5, 2)
    };
    let max_scale = isize::max(isize::max(scales.0, scales.1), scales.2);
    let max_merge = max_merge / max_scale;

    // Non-destructive read of the octree into a flat voxel map.
    let mut cells: HashMap<[isize; 3], Vector4<u8>> = HashMap::new();
    octree.for_each_leaf(|v, c| {
        cells.insert([v.x, v.y, v.z], *c);
    });
    if cells.is_empty() {
        return;
    }

    // Merge key: exact color for lossless, one shared bucket for lossy.
    let key = |c: &Vector4<u8>| -> u32 {
        if lossy {
            0
        } else {
            u32::from_le_bytes([c[0], c[1], c[2], c[3]])
        }
    };

    const N6: [[isize; 3]; 6] = [
        [1, 0, 0],
        [-1, 0, 0],
        [0, 1, 0],
        [0, -1, 0],
        [0, 0, 1],
        [0, 0, -1],
    ];

    // A cell is a region boundary if any 6-neighbor is missing or a different
    // key. Multi-source BFS from every boundary cell assigns each cell a depth;
    // the deepest cells seed bricks (racegen's interior-first ordering).
    let mut depth: HashMap<[isize; 3], u32> = HashMap::new();
    let mut queue: VecDeque<[isize; 3]> = VecDeque::new();
    for (&p, c) in &cells {
        let k = key(c);
        let boundary = N6.iter().any(|d| {
            let n = [p[0] + d[0], p[1] + d[1], p[2] + d[2]];
            cells.get(&n).map_or(true, |nc| key(nc) != k)
        });
        if boundary {
            depth.insert(p, 0);
            queue.push_back(p);
        }
    }
    while let Some(p) = queue.pop_front() {
        let d = depth[&p];
        let pk = key(&cells[&p]);
        for off in N6 {
            let n = [p[0] + off[0], p[1] + off[1], p[2] + off[2]];
            if cells.get(&n).map_or(false, |nc| key(nc) == pk) && !depth.contains_key(&n) {
                depth.insert(n, d + 1);
                queue.push_back(n);
            }
        }
    }

    // Deepest first; coordinate order breaks ties for deterministic output.
    let mut seeds: Vec<[isize; 3]> = cells.keys().copied().collect();
    seeds.sort_by(|a, b| {
        depth[b]
            .cmp(&depth[a])
            .then(a[0].cmp(&b[0]))
            .then(a[1].cmp(&b[1]))
            .then(a[2].cmp(&b[2]))
    });

    let mut used: HashSet<[isize; 3]> = HashSet::new();

    // The six growth candidates as (axis, sign).
    const DIRS: [(usize, isize); 6] =
        [(0, -1), (0, 1), (1, -1), (1, 1), (2, -1), (2, 1)];

    for seed in seeds {
        if used.contains(&seed) {
            continue;
        }
        let k = key(&cells[&seed]);
        let mut lo = seed;
        let mut hi = seed;

        loop {
            let dims = [
                hi[0] - lo[0] + 1,
                hi[1] - lo[1] + 1,
                hi[2] - lo[2] + 1,
            ];
            // Try the shortest extendable side first (ties: DIRS order, i.e.
            // negative before positive, x before y before z).
            let mut cand: Vec<(isize, usize, isize)> = DIRS
                .iter()
                .filter(|&&(axis, _)| dims[axis] < max_merge)
                .map(|&(axis, sign)| (dims[axis], axis, sign))
                .collect();
            cand.sort_by_key(|&(len, axis, sign)| (len, axis, sign));

            let mut grew = false;
            for (_, axis, sign) in cand {
                // The plane one step past the current side in this direction.
                let plane = if sign > 0 { hi[axis] + 1 } else { lo[axis] - 1 };
                let (a, b) = ((axis + 1) % 3, (axis + 2) % 3);
                let face_ok = (lo[a]..=hi[a]).all(|ca| {
                    (lo[b]..=hi[b]).all(|cb| {
                        let mut p = [0isize; 3];
                        p[axis] = plane;
                        p[a] = ca;
                        p[b] = cb;
                        cells.get(&p).map_or(false, |c| key(c) == k) && !used.contains(&p)
                    })
                });
                if face_ok {
                    if sign > 0 {
                        hi[axis] += 1;
                    } else {
                        lo[axis] -= 1;
                    }
                    grew = true;
                    break;
                }
            }
            if !grew {
                break;
            }
        }

        // Claim the box and emit one brick.
        let mut colors = Vec::new();
        for x in lo[0]..=hi[0] {
            for y in lo[1]..=hi[1] {
                for z in lo[2]..=hi[2] {
                    let p = [x, y, z];
                    used.insert(p);
                    if lossy {
                        colors.push(cells[&p]);
                    }
                }
            }
        }
        let color = if lossy {
            rgba_to_brick_color(average_rgba(&colors))
        } else {
            rgba_to_brick_color(cells[&seed])
        };

        let width = hi[0] - lo[0] + 1;
        let height = hi[1] - lo[1] + 1;
        let depth_z = hi[2] - lo[2] + 1;
        save_data.bricks.push(create_brick(
            opts,
            scales,
            (width, depth_z, height),
            (lo[0], lo[2], lo[1]),
            color,
        ));
    }
}

fn create_brick(
    opts: &ConvertOptions,
    scale: (isize, isize, isize),
    size: (isize, isize, isize),
    pos: (isize, isize, isize),
    color: Color,
) -> Brick {
    let brick_size = BrickSize::new(
        (scale.0 * size.0) as u16,
        (scale.1 * size.1) as u16,
        (scale.2 * size.2) as u16,
    );

    let position = Position {
        x: (scale.0 * size.0 + 2 * scale.0 * pos.0) as i32,
        y: (scale.1 * size.1 + 2 * scale.1 * pos.1) as i32,
        z: (scale.2 * size.2 + 2 * scale.2 * pos.2) as i32,
    };

    let asset_name = if opts.bricktype == BrickType::Microbricks {
        "PB_DefaultMicroBrick"
    } else if opts.bricktype == BrickType::Tiles {
        "PB_DefaultTile"
    } else {
        "PB_DefaultBrick"
    };

    let brick_type = BrdbBrickType::from((asset_name, brick_size));

    Brick {
        id: None,
        asset: brick_type,
        owner_index: None,
        original_owner_index: None,
        position,
        rotation: Rotation::Deg0,
        direction: Direction::ZPositive,
        collision: brdb::Collision {
            player: opts.player_collision,
            physics: opts.physics_collision,
            ..Default::default()
        },
        visible: true,
        color,
        material: match opts.material {
            crate::Material::Plastic => "BMC_Plastic".into(),
            crate::Material::Glass => "BMC_Glass".into(),
            crate::Material::Glow => "BMC_Glow".into(),
            crate::Material::Metallic => "BMC_Metallic".into(),
            crate::Material::Hologram => "BMC_Hologram".into(),
            crate::Material::Ghost => "BMC_Ghost".into(),
        },
        material_intensity: opts.material_intensity as u8,
        components: Vec::new(),
    }
}

fn average_rgba(colors: &[Vector4<u8>]) -> Vector4<u8> {
    if colors.is_empty() {
        return Vector4::new(255, 255, 255, 255);
    }

    let sums = colors.iter().fold([0_u32; 4], |mut sums, color| {
        for (index, sum) in sums.iter_mut().enumerate() {
            *sum += u32::from(color[index]);
        }
        sums
    });
    let count = colors.len() as u32;
    Vector4::new(
        ((sums[0] + count / 2) / count) as u8,
        ((sums[1] + count / 2) / count) as u8,
        ((sums[2] + count / 2) / count) as u8,
        ((sums[3] + count / 2) / count) as u8,
    )
}

fn rgba_to_brick_color(color: Vector4<u8>) -> Color {
    Color::new(color[0], color[1], color[2])
}
