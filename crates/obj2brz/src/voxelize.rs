use crate::barycentric::interpolate_uv;
use crate::BrickType;
use crate::intersect::intersect;
use crate::octree::{Branches, TreeBody, VoxelTree};


use cgmath::{Vector2, Vector3, Vector4};
use image::RgbaImage;

#[derive(Debug, Copy, Clone)]
#[repr(C)]
struct Triangle {
    material_id: Option<usize>,
    vertices: [Vector3<f32>; 3],
    uvs: Option<[Vector2<f32>; 3]>,
}

pub fn voxelize(
    models: &[tobj::Model],
    materials: &[RgbaImage],
    _scale: f32,
    _bricktype: BrickType,
    material_filter: Option<usize>,
) -> VoxelTree<Vector4<u8>> {
    let mut octree = VoxelTree::<Vector4<u8>>::new();

    // Determine model AABB to expand triangle octree to final size. OBJ files
    // may contain empty groups; never assume the first model has geometry.
    let Some(first_positions) = models
        .iter()
        .map(|model| &model.mesh.positions)
        .find(|positions| positions.len() >= 3)
    else {
        return octree;
    };
    let mut min = Vector3::new(first_positions[0], first_positions[1], first_positions[2]);
    let mut max = min;

    for m in models.iter() {
        let p = &m.mesh.positions;
        for v in (0..p.len() / 3 * 3).step_by(3) {
            for m in 0..3 {
                min[m] = min[m].min(p[v + m]);
                max[m] = max[m].max(p[v + m]);
            }
        }
    }

    let floor_min = Vector3::<isize>::new(
        min[0].floor() as isize - 1,
        min[1].floor() as isize - 1,
        min[2].floor() as isize - 1,
    );
    let ceil_max = Vector3::<isize>::new(
        max[0].ceil() as isize + 1,
        max[1].ceil() as isize + 1,
        max[2].ceil() as isize + 1,
    );

    while !octree.contains_bounds(floor_min) || !octree.contains_bounds(ceil_max) {
        octree.size += 1;
    }

    let mask = 1 << octree.size;

    // Voxelize
    let mut triangles = Vec::<Triangle>::new();
    for m in models.iter() {
        let mesh = &m.mesh;
        let material = mesh.material_id;

        // Skip if material doesn't match filter
        if let Some(filter_id) = material_filter {
            if material != Some(filter_id) {
                continue;
            }
        }

        for indices in mesh.indices.chunks_exact(3) {
            let vertex = |index: u32| {
                let index = index as usize * 3;
                mesh.positions
                    .get(index..index + 3)
                    .map(|position| Vector3::new(position[0], position[1], position[2]))
            };
            let (Some(v0), Some(v1), Some(v2)) =
                (vertex(indices[0]), vertex(indices[1]), vertex(indices[2]))
            else {
                // A malformed face should not take the whole conversion down.
                continue;
            };

            let uv = |index: u32| {
                let index = index as usize * 2;
                mesh.texcoords
                    .get(index..index + 2)
                    .map(|coords| Vector2::new(coords[0], coords[1]))
            };
            let uvs = match (uv(indices[0]), uv(indices[1]), uv(indices[2])) {
                (Some(uv0), Some(uv1), Some(uv2)) => Some([uv0, uv1, uv2]),
                _ => None,
            };

            let triangle = Triangle {
                material_id: material,
                vertices: [v0, v1, v2],
                uvs,
            };

            triangles.push(triangle);
        }
    }

    recursive_voxelize(&mut octree.contents, mask, triangles, materials);

    octree
}

fn recursive_voxelize(
    branches: &mut Branches<Vector4<u8>>,
    mask: isize,
    vector: Vec<Triangle>,
    materials: &[RgbaImage],
) {
    let m = mask >> 1;
    let half_box = (2 * m + ((m == 0) as isize)) as f32 / 2.;

    for (i, branch) in branches.iter_mut().enumerate() {
        if let TreeBody::Empty = branch {
            let center = Vector3::<f32>::new(
                half_box * (2 * ((i & 4) > 0) as isize - 1) as f32,
                half_box * (2 * ((i & 2) > 0) as isize - 1) as f32,
                half_box * (2 * ((i & 1) > 0) as isize - 1) as f32,
            );

            let mut triangles = Vec::<Triangle>::new();
            let mut colors = Vec::<Vector4<u8>>::new();

            for triangle in &vector {
                match intersect(
                    half_box,
                    center,
                    triangle.vertices[0],
                    triangle.vertices[1],
                    triangle.vertices[2],
                ) {
                    Some(intersection) => {
                        // Only calculate colors if in root level
                        if m == 0 {
                            // Missing material assignments and material IDs that
                            // do not resolve are common in exported OBJ files.
                            // Use the first (default) material rather than
                            // silently generating black/NaN-colored bricks.
                            let material = triangle
                                .material_id
                                .and_then(|id| materials.get(id))
                                .or_else(|| materials.first());
                            if let Some(material) = material {
                                let uv =
                                    interpolate_uv(&triangle.vertices, &triangle.uvs, intersection);
                                let width = material.width().saturating_sub(1);
                                let height = material.height().saturating_sub(1);
                                let u = ((uv[0] - uv[0].floor()) * width as f32) as u32;
                                let v = ((1. - uv[1] + uv[1].floor()) * height as f32) as u32;

                                let c = *material.get_pixel(u, v);
                                if c[3] == 0 {
                                    continue;
                                }
                                colors.push(Vector4::<u8>::new(c[0], c[1], c[2], c[3]));
                            }
                        }
                    }
                    None => continue,
                }

                let mut cloned_triangle = *triangle;
                cloned_triangle.vertices[0] -= center;
                cloned_triangle.vertices[1] -= center;
                cloned_triangle.vertices[2] -= center;

                triangles.push(cloned_triangle);
            }

            if triangles.is_empty() {
                continue;
            }
            if m != 0 {
                // Not yet at root level, keep on recursing...
                *branch = TreeBody::Branch(Box::new(TreeBody::empty()));
                if let TreeBody::Branch(b) = branch {
                    recursive_voxelize(b, m, triangles, materials);
                }
            } else {
                *branch = TreeBody::Leaf(average_rgba(&colors));
            }
        }
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
