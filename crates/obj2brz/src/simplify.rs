use crate::octree::{TreeBody, VoxelTree};
use crate::{BrickType, ConvertOptions, SaveData};

use brdb::{Brick, BrickSize, BrickType as BrdbBrickType, Color, Direction, Position, Rotation};
use cgmath::{Vector3, Vector4};

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
