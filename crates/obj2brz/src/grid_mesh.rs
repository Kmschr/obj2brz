//! Experimental triangle-plane conversion.
//!
//! Each source face is represented by one right-triangle micro-wedge, or by
//! two wedges after dropping an altitude to the face's longest edge. Coplanar
//! wedges share a frozen grid whenever their in-plane axes differ by a 90°
//! rotation and their centers remain integral in that grid's local frame.

use crate::brdb_support;
use crate::error::{ConversionError, ConversionResult};
use crate::{ConvertOptions, Material};
use brdb::{
    assets, Brick, BrickSize, BrickType, Collision, Color, Entity, Quat4f, Rotation, Vector3f,
};
use cgmath::{InnerSpace, Vector2, Vector3};
use image::RgbaImage;
use std::collections::BTreeMap;

const GAME_UNITS_PER_STUD: f32 = 10.0;
const RIGHT_ANGLE_COS_EPSILON: f32 = 1.0e-5;
const DEGENERATE_RELATIVE_EPSILON: f32 = 1.0e-12;
const FACE_TEXTURE_SUBDIVISIONS: u32 = 8;
const AXIS_ALIGNMENT_EPSILON: f32 = 1.0e-3;
// Brick positions are integral within a grid. Nearest-integer placement can
// move a fitted center by at most half a game unit on each in-plane axis.
const LOCAL_POSITION_EPSILON: f32 = 5.01e-1;
const COPLANAR_DISTANCE_EPSILON: f32 = 5.0e-2;
const PLANE_NORMAL_KEY_SCALE: f32 = 1_000.0;
const PLANE_DISTANCE_KEY_SCALE: f32 = 10.0;
const VERTEX_KEY_SCALE: f32 = 1_000.0;
const QUAD_PROJECTION_EPSILON: f32 = 1.0e-5;
// Covers the wedge's two-unit thickness plus the worst-case endpoint growth
// from clamping a very short leg to the minimum two-unit procedural length.
const ANCHOR_PADDING: f32 = 5.0;

pub(crate) struct GridMeshData {
    pub anchor: Brick,
    pub grids: Vec<(Entity, Vec<Brick>)>,
    pub stats: GridMeshStats,
}

#[derive(Debug, Default, Clone)]
pub(crate) struct GridMeshStats {
    pub source_triangles: usize,
    pub subdivided_source_triangles: usize,
    pub emitted_wedges: usize,
    pub emitted_grids: usize,
    pub skipped_degenerate_triangles: usize,
    pub skipped_transparent_triangles: usize,
    endpoint_squared_error_sum: f64,
    endpoint_sample_count: usize,
    pub max_endpoint_error: f32,
}

impl GridMeshStats {
    pub fn rms_endpoint_error(&self) -> f32 {
        if self.endpoint_sample_count == 0 {
            0.0
        } else {
            (self.endpoint_squared_error_sum / self.endpoint_sample_count as f64).sqrt() as f32
        }
    }

    fn record_wedge(&mut self, b_error: f32, c_error: f32) {
        self.emitted_wedges += 1;
        for error in [b_error, c_error] {
            self.endpoint_squared_error_sum += f64::from(error) * f64::from(error);
            self.endpoint_sample_count += 1;
            self.max_endpoint_error = self.max_endpoint_error.max(error);
        }
    }
}

struct FittedWedge {
    center: Vector3<f32>,
    x_axis: Vector3<f32>,
    y_axis: Vector3<f32>,
    z_axis: Vector3<f32>,
    endpoint_errors: [Vector3<f32>; 2],
    brick: Brick,
}

struct SourceTriangle {
    points: [Vector3<f32>; 3],
    normal: Vector3<f32>,
    color: Color,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct VertexKey([i64; 3]);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct EdgeKey(VertexKey, VertexKey);

#[derive(Debug, Clone, Copy)]
struct FaceEdge {
    face_index: usize,
    opposite_index: usize,
    start_index: usize,
    end_index: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct PlaneKey([i64; 4]);

struct GridGroup {
    center: Vector3<f32>,
    x_axis: Vector3<f32>,
    y_axis: Vector3<f32>,
    z_axis: Vector3<f32>,
    bricks: Vec<Brick>,
}

pub(crate) fn build(
    models: &[tobj::Model],
    materials: &[RgbaImage],
    opts: &ConvertOptions,
) -> ConversionResult<GridMeshData> {
    let scale = if crate::ldraw::is_ldraw_path(&opts.input_file_path) {
        GAME_UNITS_PER_STUD
    } else {
        GAME_UNITS_PER_STUD * opts.scale
    };

    let (min, max, vertical_offset) = transformed_bounds(models, scale)?;
    let mut stats = GridMeshStats::default();
    let mut wedges = Vec::new();
    let mut source_faces = Vec::new();
    let material_averages: Vec<_> = materials.iter().map(average_image_color).collect();

    for model in models {
        let mesh = &model.mesh;
        for indices in mesh.indices.chunks_exact(3) {
            let Some(source_points) = triangle_positions(mesh, indices) else {
                stats.skipped_degenerate_triangles += 1;
                continue;
            };
            let points = source_points.map(|point| transform_point(point, scale, vertical_offset));
            let normal = (points[1] - points[0]).cross(points[2] - points[0]);
            let longest_squared = longest_edge_squared(points);
            if !normal.magnitude2().is_finite()
                || longest_squared <= 0.0
                || normal.magnitude2()
                    <= longest_squared * longest_squared * DEGENERATE_RELATIVE_EPSILON
            {
                stats.skipped_degenerate_triangles += 1;
                continue;
            }

            let uvs = triangle_uvs(mesh, indices);
            let material_index = mesh
                .material_id
                .filter(|&index| index < materials.len())
                .unwrap_or(0);
            let color = if let Some(texture) = materials.get(material_index) {
                if let Some(uvs) = uvs {
                    average_triangle_texture(texture, uvs)
                } else {
                    material_averages.get(material_index).copied().flatten()
                }
            } else {
                Some(Color::new(255, 255, 255))
            };
            let Some(color) = color else {
                stats.skipped_transparent_triangles += 1;
                continue;
            };

            stats.source_triangles += 1;
            source_faces.push(SourceTriangle {
                points,
                normal: normal.normalize(),
                color,
            });
        }
    }
    add_model_triangles(&mut wedges, &source_faces, opts, &mut stats)?;

    if stats.source_triangles == 0 {
        return Err(ConversionError::ObjParseError(
            "grid mesh contains no visible, non-degenerate triangle faces".to_string(),
        ));
    }

    let grids = group_coplanar_wedges(wedges, &mut stats);
    stats.emitted_grids = grids.len();
    let anchor = bounds_anchor(
        transform_point(min, scale, vertical_offset),
        transform_point(max, scale, vertical_offset),
    )?;

    Ok(GridMeshData {
        anchor,
        grids,
        stats,
    })
}

pub(crate) fn write(opts: &ConvertOptions, data: GridMeshData) -> ConversionResult<()> {
    log_stats(opts, &data.stats);
    let output = crate::output_file_path(opts);
    brdb_support::write_grid_mesh(
        output.clone(),
        data.anchor,
        data.grids,
        &data.stats,
        opts,
        Some(crate::convert::obj_preview_jpg()?),
    )?;
    opts.logger.log(format!("Save written to: {:?}", output));
    Ok(())
}

pub(crate) fn brz_bytes(opts: &ConvertOptions, data: GridMeshData) -> ConversionResult<Vec<u8>> {
    log_stats(opts, &data.stats);
    brdb_support::grid_mesh_brz_bytes(
        &opts.save_name,
        data.anchor,
        data.grids,
        &data.stats,
        opts,
        Some(crate::convert::obj_preview_jpg()?),
    )
}

fn log_stats(opts: &ConvertOptions, stats: &GridMeshStats) {
    opts.logger.log(format!(
        "Grid mesh: {} source faces, {} subdivided, {} wedges on {} frozen grids",
        stats.source_triangles,
        stats.subdivided_source_triangles,
        stats.emitted_wedges,
        stats.emitted_grids
    ));
    let saved_grids = stats.emitted_wedges.saturating_sub(stats.emitted_grids);
    if saved_grids > 0 {
        opts.logger.log(format!(
            "Coplanar grouping saved {} grids ({:.1}% reduction)",
            saved_grids,
            saved_grids as f64 * 100.0 / stats.emitted_wedges as f64
        ));
    }
    opts.logger.log(format!(
        "Endpoint error: RMS {:.3}, maximum {:.3} Brickadia units",
        stats.rms_endpoint_error(),
        stats.max_endpoint_error
    ));
    if stats.skipped_degenerate_triangles > 0 {
        opts.logger.log(format!(
            "Skipped {} degenerate triangle faces",
            stats.skipped_degenerate_triangles
        ));
    }
    if stats.skipped_transparent_triangles > 0 {
        opts.logger.log(format!(
            "Skipped {} fully transparent triangle faces",
            stats.skipped_transparent_triangles
        ));
    }
    if stats.emitted_grids > 2000 {
        opts.logger.log(format!(
            "⚠ Grid mesh generated {} grids; prefabs above 2,000 grids may have trouble loading in game.",
            stats.emitted_grids
        ));
    }
}

fn transformed_bounds(
    models: &[tobj::Model],
    scale: f32,
) -> ConversionResult<(Vector3<f32>, Vector3<f32>, f32)> {
    let mut min = Vector3::new(f32::INFINITY, f32::INFINITY, f32::INFINITY);
    let mut max = Vector3::new(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);

    for model in models {
        for position in model.mesh.positions.chunks_exact(3) {
            let point = Vector3::new(position[0], position[1], position[2]);
            min.x = min.x.min(point.x);
            min.y = min.y.min(point.y);
            min.z = min.z.min(point.z);
            max.x = max.x.max(point.x);
            max.y = max.y.max(point.y);
            max.z = max.z.max(point.z);
        }
    }
    if !min.x.is_finite() || !max.x.is_finite() {
        return Err(ConversionError::ObjParseError(
            "model contains no finite vertices for grid mesh conversion".to_string(),
        ));
    }

    // Source formats are normalized to Y-up. The final Brickadia Z minimum is
    // source Y * scale; rest the model on Z=0 without disturbing X/Y.
    let vertical_offset = if min.y < 0.0 { -min.y * scale } else { 0.0 };
    Ok((min, max, vertical_offset))
}

fn transform_point(point: Vector3<f32>, scale: f32, vertical_offset: f32) -> Vector3<f32> {
    Vector3::new(
        point.x * scale,
        point.z * scale,
        point.y * scale + vertical_offset,
    )
}

fn triangle_positions(mesh: &tobj::Mesh, indices: &[u32]) -> Option<[Vector3<f32>; 3]> {
    let vertex = |index: u32| {
        let start = index as usize * 3;
        mesh.positions
            .get(start..start + 3)
            .map(|p| Vector3::new(p[0], p[1], p[2]))
    };
    Some([
        vertex(indices[0])?,
        vertex(indices[1])?,
        vertex(indices[2])?,
    ])
}

fn triangle_uvs(mesh: &tobj::Mesh, indices: &[u32]) -> Option<[Vector2<f32>; 3]> {
    let uv = |index: u32| {
        let start = index as usize * 2;
        mesh.texcoords
            .get(start..start + 2)
            .map(|p| Vector2::new(p[0], p[1]))
    };
    Some([uv(indices[0])?, uv(indices[1])?, uv(indices[2])?])
}

fn longest_edge_squared(points: [Vector3<f32>; 3]) -> f32 {
    [
        (points[1] - points[0]).magnitude2(),
        (points[2] - points[1]).magnitude2(),
        (points[0] - points[2]).magnitude2(),
    ]
    .into_iter()
    .fold(0.0, f32::max)
}

fn add_model_triangles(
    wedges: &mut Vec<FittedWedge>,
    faces: &[SourceTriangle],
    opts: &ConvertOptions,
    stats: &mut GridMeshStats,
) -> ConversionResult<()> {
    let pairs = find_coplanar_quad_pairs(faces);
    let mut paired = vec![false; faces.len()];

    for (left_index, right_index, base_a, base_b) in pairs {
        let left = &faces[left_index];
        let right = &faces[right_index];
        add_triangle_on_shared_base(wedges, left, base_a, base_b, opts, stats)?;
        add_triangle_on_shared_base(wedges, right, base_a, base_b, opts, stats)?;
        paired[left_index] = true;
        paired[right_index] = true;
    }

    for (index, face) in faces.iter().enumerate() {
        if !paired[index] {
            add_target_triangle(wedges, face.points, face.normal, face.color, opts, stats)?;
        }
    }
    Ok(())
}

fn find_coplanar_quad_pairs(
    faces: &[SourceTriangle],
) -> Vec<(usize, usize, Vector3<f32>, Vector3<f32>)> {
    let mut edges: BTreeMap<EdgeKey, Vec<FaceEdge>> = BTreeMap::new();
    for (face_index, face) in faces.iter().enumerate() {
        for (start_index, end_index, opposite_index) in [(0, 1, 2), (1, 2, 0), (2, 0, 1)] {
            let start = vertex_key(face.points[start_index]);
            let end = vertex_key(face.points[end_index]);
            let key = if start <= end {
                EdgeKey(start, end)
            } else {
                EdgeKey(end, start)
            };
            edges.entry(key).or_default().push(FaceEdge {
                face_index,
                opposite_index,
                start_index,
                end_index,
            });
        }
    }

    let mut paired = vec![false; faces.len()];
    let mut pairs = Vec::new();
    for candidates in edges.into_values() {
        for left_candidate in 0..candidates.len() {
            let left_edge = candidates[left_candidate];
            if paired[left_edge.face_index] {
                continue;
            }
            for &right_edge in &candidates[left_candidate + 1..] {
                if left_edge.face_index == right_edge.face_index
                    || paired[right_edge.face_index]
                    || !edges_form_coplanar_quad(faces, left_edge, right_edge)
                {
                    continue;
                }

                let face = &faces[left_edge.face_index];
                pairs.push((
                    left_edge.face_index,
                    right_edge.face_index,
                    face.points[left_edge.start_index],
                    face.points[left_edge.end_index],
                ));
                paired[left_edge.face_index] = true;
                paired[right_edge.face_index] = true;
                break;
            }
        }
    }
    pairs
}

fn edges_form_coplanar_quad(
    faces: &[SourceTriangle],
    left_edge: FaceEdge,
    right_edge: FaceEdge,
) -> bool {
    let left = &faces[left_edge.face_index];
    let right = &faces[right_edge.face_index];
    if left.normal.dot(right.normal) < 1.0 - AXIS_ALIGNMENT_EPSILON {
        return false;
    }

    let base_a = left.points[left_edge.start_index];
    let base_b = left.points[left_edge.end_index];
    let left_opposite = left.points[left_edge.opposite_index];
    let right_opposite = right.points[right_edge.opposite_index];
    if vertex_key(left_opposite) == vertex_key(right_opposite) {
        return false;
    }

    let base = base_b - base_a;
    let base_squared = base.magnitude2();
    if !base_squared.is_finite() || base_squared <= f32::EPSILON {
        return false;
    }

    let left_distance = (left_opposite - base_a).dot(left.normal);
    let right_distance = (right_opposite - base_a).dot(left.normal);
    if left_distance.abs() > COPLANAR_DISTANCE_EPSILON
        || right_distance.abs() > COPLANAR_DISTANCE_EPSILON
    {
        return false;
    }

    let left_side = base.cross(left_opposite - base_a).dot(left.normal);
    let right_side = base.cross(right_opposite - base_a).dot(left.normal);
    if left_side * right_side >= 0.0 {
        return false;
    }

    let projections_inside = [left_opposite, right_opposite].into_iter().all(|point| {
        let projection = (point - base_a).dot(base) / base_squared;
        projection > QUAD_PROJECTION_EPSILON && projection < 1.0 - QUAD_PROJECTION_EPSILON
    });
    projections_inside && !direct_right_wedges_are_compatible(left.points, right.points)
}

fn direct_right_wedges_are_compatible(left: [Vector3<f32>; 3], right: [Vector3<f32>; 3]) -> bool {
    let (Some(left), Some(right)) = (right_wedge_frame(left), right_wedge_frame(right)) else {
        return false;
    };
    if quarter_turn(left.1, left.2, right.1, right.2).is_none() {
        return false;
    }

    let delta = right.0 - left.0;
    let local_x = delta.dot(left.1);
    let local_y = delta.dot(left.2);
    let local_z = delta.dot(left.3);
    local_z.abs() <= COPLANAR_DISTANCE_EPSILON
        && (local_x - local_x.round()).abs() <= LOCAL_POSITION_EPSILON
        && (local_y - local_y.round()).abs() <= LOCAL_POSITION_EPSILON
}

fn right_wedge_frame(
    points: [Vector3<f32>; 3],
) -> Option<(Vector3<f32>, Vector3<f32>, Vector3<f32>, Vector3<f32>)> {
    for right_index in 0..3 {
        let a = points[right_index];
        let ab = points[(right_index + 1) % 3] - a;
        let ac = points[(right_index + 2) % 3] - a;
        if ab.dot(ac).abs() > RIGHT_ANGLE_COS_EPSILON * ab.magnitude() * ac.magnitude() {
            continue;
        }

        let x_axis = ab.normalize();
        let rejected = ac - x_axis * ac.dot(x_axis);
        if !rejected.magnitude2().is_finite() || rejected.magnitude2() <= f32::EPSILON {
            return None;
        }
        let y_axis = rejected.normalize();
        let z_axis = x_axis.cross(y_axis).normalize();
        let x_length = (ab.magnitude() * 0.5).round().max(1.0) * 2.0;
        let y_length = (ac.dot(y_axis).abs() * 0.5).round().max(1.0) * 2.0;
        let center = a + x_axis * (x_length * 0.5) + y_axis * (y_length * 0.5);
        return Some((center, x_axis, y_axis, z_axis));
    }
    None
}

fn vertex_key(point: Vector3<f32>) -> VertexKey {
    VertexKey([
        (point.x * VERTEX_KEY_SCALE).round() as i64,
        (point.y * VERTEX_KEY_SCALE).round() as i64,
        (point.z * VERTEX_KEY_SCALE).round() as i64,
    ])
}

fn add_triangle_on_shared_base(
    wedges: &mut Vec<FittedWedge>,
    face: &SourceTriangle,
    base_a: Vector3<f32>,
    base_b: Vector3<f32>,
    opts: &ConvertOptions,
    stats: &mut GridMeshStats,
) -> ConversionResult<()> {
    let base = base_b - base_a;
    let opposite = face
        .points
        .into_iter()
        .max_by(|left, right| {
            distance_from_line_squared(*left, base_a, base)
                .total_cmp(&distance_from_line_squared(*right, base_a, base))
        })
        .expect("a triangle always has three points");
    let projection = (opposite - base_a).dot(base) / base.magnitude2();
    let foot = base_a + base * projection;

    stats.subdivided_source_triangles += 1;
    let (b0, c0) = preserve_winding(foot, base_a, opposite, face.normal);
    add_right_triangle(wedges, foot, b0, c0, face.color, opts)?;
    let (b1, c1) = preserve_winding(foot, opposite, base_b, face.normal);
    add_right_triangle(wedges, foot, b1, c1, face.color, opts)
}

fn distance_from_line_squared(
    point: Vector3<f32>,
    line_start: Vector3<f32>,
    line: Vector3<f32>,
) -> f32 {
    let offset = point - line_start;
    let rejection = offset - line * (offset.dot(line) / line.magnitude2());
    rejection.magnitude2()
}

fn add_target_triangle(
    wedges: &mut Vec<FittedWedge>,
    points: [Vector3<f32>; 3],
    source_normal: Vector3<f32>,
    color: Color,
    opts: &ConvertOptions,
    stats: &mut GridMeshStats,
) -> ConversionResult<()> {
    for right_index in 0..3 {
        let a = points[right_index];
        let b = points[(right_index + 1) % 3];
        let c = points[(right_index + 2) % 3];
        let ab = b - a;
        let ac = c - a;
        if ab.dot(ac).abs() <= RIGHT_ANGLE_COS_EPSILON * ab.magnitude() * ac.magnitude() {
            return add_right_triangle(wedges, a, b, c, color, opts);
        }
    }

    stats.subdivided_source_triangles += 1;
    let edges = [
        (
            points[0],
            points[1],
            points[2],
            (points[1] - points[0]).magnitude2(),
        ),
        (
            points[1],
            points[2],
            points[0],
            (points[2] - points[1]).magnitude2(),
        ),
        (
            points[2],
            points[0],
            points[1],
            (points[0] - points[2]).magnitude2(),
        ),
    ];
    let &(base_a, base_b, opposite, _) = edges
        .iter()
        .max_by(|left, right| left.3.total_cmp(&right.3))
        .expect("a triangle always has three edges");
    let base = base_b - base_a;
    let t = (opposite - base_a).dot(base) / base.magnitude2();
    let foot = base_a + base * t.clamp(0.0, 1.0);

    let (b0, c0) = preserve_winding(foot, base_a, opposite, source_normal);
    add_right_triangle(wedges, foot, b0, c0, color, opts)?;
    let (b1, c1) = preserve_winding(foot, opposite, base_b, source_normal);
    add_right_triangle(wedges, foot, b1, c1, color, opts)
}

fn preserve_winding(
    a: Vector3<f32>,
    mut b: Vector3<f32>,
    mut c: Vector3<f32>,
    source_normal: Vector3<f32>,
) -> (Vector3<f32>, Vector3<f32>) {
    if (b - a).cross(c - a).dot(source_normal) < 0.0 {
        std::mem::swap(&mut b, &mut c);
    }
    (b, c)
}

fn add_right_triangle(
    wedges: &mut Vec<FittedWedge>,
    a: Vector3<f32>,
    b: Vector3<f32>,
    c: Vector3<f32>,
    color: Color,
    opts: &ConvertOptions,
) -> ConversionResult<()> {
    let ab = b - a;
    let ac = c - a;
    let x_axis = ab.normalize();
    let rejected = ac - x_axis * ac.dot(x_axis);
    if !rejected.magnitude2().is_finite() || rejected.magnitude2() <= f32::EPSILON {
        return Err(ConversionError::ObjParseError(
            "grid mesh produced a degenerate right-triangle subdivision".to_string(),
        ));
    }
    let y_axis = rejected.normalize();
    let z_axis = x_axis.cross(y_axis).normalize();
    let x_half = half_extent(ab.magnitude(), "X")?;
    let y_half = half_extent(ac.dot(y_axis).abs(), "Y")?;
    let x_length = f32::from(x_half) * 2.0;
    let y_length = f32::from(y_half) * 2.0;
    let location = a + x_axis * (x_length * 0.5) + y_axis * (y_length * 0.5);

    wedges.push(FittedWedge {
        center: location,
        x_axis,
        y_axis,
        z_axis,
        endpoint_errors: [a + x_axis * x_length - b, a + y_axis * y_length - c],
        brick: Brick {
            asset: BrickType::Procedural {
                asset: assets::bricks::PB_DEFAULT_MICRO_WEDGE,
                size: BrickSize::new(x_half, y_half, 1),
            },
            position: (0, 0, -1).into(),
            color,
            material: material_name(opts.material).into(),
            collision: Collision {
                player: opts.player_collision,
                physics: opts.physics_collision,
                ..Default::default()
            },
            ..Default::default()
        },
    });
    Ok(())
}

fn group_coplanar_wedges(
    wedges: Vec<FittedWedge>,
    stats: &mut GridMeshStats,
) -> Vec<(Entity, Vec<Brick>)> {
    let mut planes: BTreeMap<PlaneKey, Vec<GridGroup>> = BTreeMap::new();

    for wedge in wedges {
        let key = plane_key(&wedge);
        // Quantized plane keys make lookup fast, but a mathematically shared
        // plane can land on opposite sides of a rounding boundary when its
        // two wedges were fitted from different edge pairs. Probe adjacent
        // buckets and let the stricter axis/position checks make the decision.
        let added = neighboring_plane_keys(key).into_iter().any(|candidate| {
            planes
                .get_mut(&candidate)
                .is_some_and(|groups| groups.iter_mut().any(|group| group.try_add(&wedge, stats)))
        });
        if !added {
            planes
                .entry(key)
                .or_default()
                .push(GridGroup::new(wedge, stats));
        }
    }

    planes
        .into_values()
        .flatten()
        .map(GridGroup::into_grid)
        .collect()
}

fn neighboring_plane_keys(key: PlaneKey) -> Vec<PlaneKey> {
    let mut keys = Vec::with_capacity(81);
    keys.push(key);
    for x in -1..=1 {
        for y in -1..=1 {
            for z in -1..=1 {
                for distance in -1..=1 {
                    if x == 0 && y == 0 && z == 0 && distance == 0 {
                        continue;
                    }
                    keys.push(PlaneKey([
                        key.0[0] + x,
                        key.0[1] + y,
                        key.0[2] + z,
                        key.0[3] + distance,
                    ]));
                }
            }
        }
    }
    keys
}

fn plane_key(wedge: &FittedWedge) -> PlaneKey {
    let distance = wedge.z_axis.dot(wedge.center);
    PlaneKey([
        (wedge.z_axis.x * PLANE_NORMAL_KEY_SCALE).round() as i64,
        (wedge.z_axis.y * PLANE_NORMAL_KEY_SCALE).round() as i64,
        (wedge.z_axis.z * PLANE_NORMAL_KEY_SCALE).round() as i64,
        (distance * PLANE_DISTANCE_KEY_SCALE).round() as i64,
    ])
}

impl GridGroup {
    fn new(mut wedge: FittedWedge, stats: &mut GridMeshStats) -> Self {
        stats.record_wedge(
            wedge.endpoint_errors[0].magnitude(),
            wedge.endpoint_errors[1].magnitude(),
        );
        wedge.brick.position = (0, 0, -1).into();
        wedge.brick.rotation = Rotation::Deg0;
        Self {
            center: wedge.center,
            x_axis: wedge.x_axis,
            y_axis: wedge.y_axis,
            z_axis: wedge.z_axis,
            bricks: vec![wedge.brick],
        }
    }

    fn try_add(&mut self, wedge: &FittedWedge, stats: &mut GridMeshStats) -> bool {
        let Some(rotation) = quarter_turn(self.x_axis, self.y_axis, wedge.x_axis, wedge.y_axis)
        else {
            return false;
        };

        let delta = wedge.center - self.center;
        let local_x = delta.dot(self.x_axis);
        let local_y = delta.dot(self.y_axis);
        let local_z = delta.dot(self.z_axis);
        let rounded_x = local_x.round();
        let rounded_y = local_y.round();
        if local_z.abs() > COPLANAR_DISTANCE_EPSILON
            || (local_x - rounded_x).abs() > LOCAL_POSITION_EPSILON
            || (local_y - rounded_y).abs() > LOCAL_POSITION_EPSILON
            || rounded_x < i32::MIN as f32
            || rounded_x > i32::MAX as f32
            || rounded_y < i32::MIN as f32
            || rounded_y > i32::MAX as f32
        {
            return false;
        }

        let mut brick = wedge.brick.clone();
        brick.position = (rounded_x as i32, rounded_y as i32, -1).into();
        brick.rotation = rotation;
        // Procedural micro-wedges have triangular footprints. Quantizing the
        // two legs of an altitude split can make its otherwise adjacent
        // halves overlap slightly. Brickadia rejects overlapping bricks in a
        // single grid, which used to discard one half of many single-sided
        // source faces. Keep sharing the plane, but start another compatible
        // grid whenever the fitted triangle would overlap a resident wedge.
        if self
            .bricks
            .iter()
            .any(|resident| micro_wedge_footprints_overlap(resident, &brick))
        {
            return false;
        }
        let translation = self.x_axis * (rounded_x - local_x) + self.y_axis * (rounded_y - local_y)
            - self.z_axis * local_z;
        stats.record_wedge(
            (wedge.endpoint_errors[0] + translation).magnitude(),
            (wedge.endpoint_errors[1] + translation).magnitude(),
        );
        self.bricks.push(brick);
        true
    }

    fn into_grid(self) -> (Entity, Vec<Brick>) {
        (
            Entity {
                location: vector3f(self.center),
                rotation: quat_from_basis(self.x_axis, self.y_axis, self.z_axis),
                frozen: true,
                sleeping: true,
                ..Default::default()
            },
            self.bricks,
        )
    }
}

type FootprintPoint = (i64, i64);

fn micro_wedge_footprints_overlap(left: &Brick, right: &Brick) -> bool {
    let (Some(left), Some(right)) = (micro_wedge_footprint(left), micro_wedge_footprint(right))
    else {
        // Grid meshes only emit procedural micro-wedges. Be conservative if
        // that invariant changes rather than allowing an unchecked overlap.
        return true;
    };

    // Convex-triangle separating-axis test. Merely touching along an edge or
    // at a vertex is valid; only a positive-area intersection is an overlap.
    left.into_iter()
        .zip(left.into_iter().cycle().skip(1))
        .chain(right.into_iter().zip(right.into_iter().cycle().skip(1)))
        .take(6)
        .all(|(start, end)| {
            let axis = (start.1 - end.1, end.0 - start.0);
            projection_ranges_overlap(left, right, axis)
        })
}

fn projection_ranges_overlap(
    left: [FootprintPoint; 3],
    right: [FootprintPoint; 3],
    axis: FootprintPoint,
) -> bool {
    let range = |triangle: [FootprintPoint; 3]| {
        triangle
            .map(|point| point.0 * axis.0 + point.1 * axis.1)
            .into_iter()
            .fold((i64::MAX, i64::MIN), |(min, max), value| {
                (min.min(value), max.max(value))
            })
    };
    let (left_min, left_max) = range(left);
    let (right_min, right_max) = range(right);
    left_min < right_max && right_min < left_max
}

fn micro_wedge_footprint(brick: &Brick) -> Option<[FootprintPoint; 3]> {
    let BrickType::Procedural { asset, size } = &brick.asset else {
        return None;
    };
    if asset.as_ref() != assets::bricks::PB_DEFAULT_MICRO_WEDGE.as_ref() {
        return None;
    }

    let center = (i64::from(brick.position.x), i64::from(brick.position.y));
    let x = i64::from(size.x);
    let y = i64::from(size.y);
    let offset = |dx, dy| (center.0 + dx, center.1 + dy);
    Some(match brick.rotation {
        Rotation::Deg0 => [offset(-x, -y), offset(x, -y), offset(-x, y)],
        Rotation::Deg90 => [offset(x, -y), offset(-x, -y), offset(x, y)],
        Rotation::Deg180 => [offset(x, y), offset(x, -y), offset(-x, y)],
        Rotation::Deg270 => [offset(-x, y), offset(x, y), offset(-x, -y)],
    })
}

fn quarter_turn(
    grid_x: Vector3<f32>,
    grid_y: Vector3<f32>,
    wedge_x: Vector3<f32>,
    wedge_y: Vector3<f32>,
) -> Option<Rotation> {
    let candidates = [
        (Rotation::Deg0, grid_x, grid_y),
        (Rotation::Deg90, grid_y, -grid_x),
        (Rotation::Deg180, -grid_x, -grid_y),
        (Rotation::Deg270, -grid_y, grid_x),
    ];
    candidates
        .into_iter()
        .find(|(_, expected_x, expected_y)| {
            expected_x.dot(wedge_x) >= 1.0 - AXIS_ALIGNMENT_EPSILON
                && expected_y.dot(wedge_y) >= 1.0 - AXIS_ALIGNMENT_EPSILON
        })
        .map(|(rotation, _, _)| rotation)
}

fn half_extent(length: f32, axis: &str) -> ConversionResult<u16> {
    let rounded = (length * 0.5).round().max(1.0);
    if !rounded.is_finite() || rounded > f32::from(u16::MAX) {
        return Err(ConversionError::ObjParseError(format!(
            "grid mesh {axis} leg is too long for a procedural micro-wedge"
        )));
    }
    Ok(rounded as u16)
}

fn vector3f(vector: Vector3<f32>) -> Vector3f {
    Vector3f {
        x: vector.x,
        y: vector.y,
        z: vector.z,
    }
}

fn quat_from_basis(x_axis: Vector3<f32>, y_axis: Vector3<f32>, z_axis: Vector3<f32>) -> Quat4f {
    let (m00, m01, m02) = (x_axis.x, y_axis.x, z_axis.x);
    let (m10, m11, m12) = (x_axis.y, y_axis.y, z_axis.y);
    let (m20, m21, m22) = (x_axis.z, y_axis.z, z_axis.z);
    let trace = m00 + m11 + m22;
    let q = if trace > 0.0 {
        let s = (trace + 1.0).sqrt() * 2.0;
        Quat4f {
            w: 0.25 * s,
            x: (m21 - m12) / s,
            y: (m02 - m20) / s,
            z: (m10 - m01) / s,
        }
    } else if m00 > m11 && m00 > m22 {
        let s = (1.0 + m00 - m11 - m22).sqrt() * 2.0;
        Quat4f {
            w: (m21 - m12) / s,
            x: 0.25 * s,
            y: (m01 + m10) / s,
            z: (m02 + m20) / s,
        }
    } else if m11 > m22 {
        let s = (1.0 + m11 - m00 - m22).sqrt() * 2.0;
        Quat4f {
            w: (m02 - m20) / s,
            x: (m01 + m10) / s,
            y: 0.25 * s,
            z: (m12 + m21) / s,
        }
    } else {
        let s = (1.0 + m22 - m00 - m11).sqrt() * 2.0;
        Quat4f {
            w: (m10 - m01) / s,
            x: (m02 + m20) / s,
            y: (m12 + m21) / s,
            z: 0.25 * s,
        }
    };
    let magnitude = (q.x * q.x + q.y * q.y + q.z * q.z + q.w * q.w).sqrt();
    Quat4f {
        x: q.x / magnitude,
        y: q.y / magnitude,
        z: q.z / magnitude,
        w: q.w / magnitude,
    }
}

fn bounds_anchor(min: Vector3<f32>, max: Vector3<f32>) -> ConversionResult<Brick> {
    let center = (min + max) * 0.5;
    let rounded_center = Vector3::new(center.x.round(), center.y.round(), center.z.round());
    let half = Vector3::new(
        (max.x - rounded_center.x)
            .abs()
            .max((min.x - rounded_center.x).abs())
            + ANCHOR_PADDING,
        (max.y - rounded_center.y)
            .abs()
            .max((min.y - rounded_center.y).abs())
            + ANCHOR_PADDING,
        (max.z - rounded_center.z)
            .abs()
            .max((min.z - rounded_center.z).abs())
            + ANCHOR_PADDING,
    );
    let size = BrickSize::new(
        anchor_half_extent(half.x, "X")?,
        anchor_half_extent(half.y, "Y")?,
        anchor_half_extent(half.z, "Z")?,
    );
    let position = (
        checked_position(rounded_center.x, "X")?,
        checked_position(rounded_center.y, "Y")?,
        checked_position(rounded_center.z, "Z")?,
    );

    Ok(Brick {
        asset: BrickType::Procedural {
            asset: assets::bricks::PB_DEFAULT_MICRO_BRICK,
            size,
        },
        position: position.into(),
        visible: false,
        collision: no_collision(),
        ..Default::default()
    })
}

fn anchor_half_extent(value: f32, axis: &str) -> ConversionResult<u16> {
    let value = value.ceil().max(1.0);
    if !value.is_finite() || value > f32::from(u16::MAX) {
        return Err(ConversionError::ObjParseError(format!(
            "grid mesh is too large on {axis} for its prefab-bounds anchor"
        )));
    }
    Ok(value as u16)
}

fn checked_position(value: f32, axis: &str) -> ConversionResult<i32> {
    if !value.is_finite() || value < i32::MIN as f32 || value > i32::MAX as f32 {
        return Err(ConversionError::ObjParseError(format!(
            "grid mesh {axis} position is outside Brickadia's integer range"
        )));
    }
    Ok(value as i32)
}

fn no_collision() -> Collision {
    Collision {
        player: false,
        player1: Some(false),
        player2: Some(false),
        player3: Some(false),
        weapon: false,
        interact: false,
        tool: false,
        physics: false,
    }
}

fn material_name(material: Material) -> &'static str {
    match material {
        Material::Plastic => "BMC_Plastic",
        Material::Glass => "BMC_Glass",
        Material::Glow => "BMC_Glow",
        Material::Metallic => "BMC_Metallic",
        Material::Hologram => "BMC_Hologram",
        Material::Ghost => "BMC_Ghost",
    }
}

fn average_image_color(image: &RgbaImage) -> Option<Color> {
    average_pixels(image.pixels().map(|pixel| pixel.0))
}

fn average_triangle_texture(image: &RgbaImage, uvs: [Vector2<f32>; 3]) -> Option<Color> {
    let mut samples = Vec::with_capacity(
        ((FACE_TEXTURE_SUBDIVISIONS + 1) * (FACE_TEXTURE_SUBDIVISIONS + 2) / 2) as usize,
    );
    let divisions = FACE_TEXTURE_SUBDIVISIONS as f32;
    for i in 0..=FACE_TEXTURE_SUBDIVISIONS {
        for j in 0..=(FACE_TEXTURE_SUBDIVISIONS - i) {
            let b = i as f32 / divisions;
            let c = j as f32 / divisions;
            let a = 1.0 - b - c;
            let uv = uvs[0] * a + uvs[1] * b + uvs[2] * c;
            samples.push(sample_texture(image, uv));
        }
    }
    average_pixels(samples)
}

fn sample_texture(image: &RgbaImage, uv: Vector2<f32>) -> [u8; 4] {
    let width = image.width();
    let height = image.height();
    if width == 0 || height == 0 {
        return [0, 0, 0, 0];
    }
    let wrapped_u = uv.x.rem_euclid(1.0);
    let wrapped_v = uv.y.rem_euclid(1.0);
    let u = (wrapped_u * width as f32).floor() as u32;
    // OBJ V=0 is the bottom of the image. Clamp the exact repeat boundary to
    // the last row while retaining repeat wrapping for V outside [0, 1].
    let v = ((1.0 - wrapped_v) * height as f32)
        .floor()
        .min((height - 1) as f32) as u32;
    image.get_pixel(u, v).0
}

fn average_pixels(pixels: impl IntoIterator<Item = [u8; 4]>) -> Option<Color> {
    let mut rgb_sums = [0_u64; 3];
    let mut alpha_sum = 0_u64;
    for pixel in pixels {
        let alpha = u64::from(pixel[3]);
        alpha_sum += alpha;
        for channel in 0..3 {
            rgb_sums[channel] += u64::from(pixel[channel]) * alpha;
        }
    }
    if alpha_sum == 0 {
        return None;
    }
    Some(Color::new(
        ((rgb_sums[0] + alpha_sum / 2) / alpha_sum) as u8,
        ((rgb_sums[1] + alpha_sum / 2) / alpha_sum) as u8,
        ((rgb_sums[2] + alpha_sum / 2) / alpha_sum) as u8,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options() -> ConvertOptions {
        ConvertOptions {
            grid_mesh: true,
            ..ConvertOptions::default()
        }
    }

    fn model(points: &[[f32; 3]], indices: &[u32]) -> tobj::Model {
        let mesh = tobj::Mesh {
            positions: points.iter().flatten().copied().collect(),
            indices: indices.to_vec(),
            ..Default::default()
        };
        tobj::Model::new(mesh, "test".to_string())
    }

    fn assert_grids_have_no_overlapping_wedges(data: &GridMeshData) {
        for (_, bricks) in &data.grids {
            for left in 0..bricks.len() {
                for right in left + 1..bricks.len() {
                    assert!(
                        !micro_wedge_footprints_overlap(&bricks[left], &bricks[right]),
                        "grid contains overlapping wedges at indices {left} and {right}"
                    );
                }
            }
        }
    }

    #[test]
    fn right_triangle_uses_one_grid_and_a_hidden_anchor() {
        let data = build(
            &[model(
                &[[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 0.0, 3.0]],
                &[0, 1, 2],
            )],
            &[RgbaImage::from_pixel(1, 1, image::Rgba([10, 20, 30, 255]))],
            &options(),
        )
        .unwrap();

        assert_eq!(data.stats.source_triangles, 1);
        assert_eq!(data.stats.emitted_wedges, 1);
        assert_eq!(data.stats.emitted_grids, 1);
        assert!(!data.anchor.visible);
        assert!(!data.anchor.collision.player);
    }

    #[test]
    fn general_triangle_puts_both_subdivision_wedges_on_one_grid() {
        let data = build(
            &[model(
                &[[0.0, 0.0, 0.0], [2.0, 0.5, 0.0], [0.5, 0.0, 1.0]],
                &[0, 1, 2],
            )],
            &[RgbaImage::from_pixel(1, 1, image::Rgba([255; 4]))],
            &options(),
        )
        .unwrap();

        assert_eq!(data.stats.subdivided_source_triangles, 1);
        assert_eq!(data.stats.emitted_wedges, 2);
        assert_eq!(data.grids.len(), 1);
        for (entity, bricks) in &data.grids {
            let q = entity.rotation;
            let magnitude = (q.x * q.x + q.y * q.y + q.z * q.z + q.w * q.w).sqrt();
            assert!((magnitude - 1.0).abs() < 1.0e-5);
            assert!(entity.frozen && entity.sleeping);
            assert_eq!(bricks.len(), 2);
            assert!(bricks
                .iter()
                .any(|brick| matches!(brick.rotation, Rotation::Deg90 | Rotation::Deg270)));
        }
        assert_grids_have_no_overlapping_wedges(&data);
    }

    #[test]
    fn touching_complementary_wedges_do_not_count_as_overlapping() {
        let wedge = |rotation| Brick {
            asset: BrickType::Procedural {
                asset: assets::bricks::PB_DEFAULT_MICRO_WEDGE,
                size: BrickSize::new(7, 5, 1),
            },
            position: (10, 20, -1).into(),
            rotation,
            ..Default::default()
        };

        assert!(!micro_wedge_footprints_overlap(
            &wedge(Rotation::Deg0),
            &wedge(Rotation::Deg180)
        ));
        assert!(micro_wedge_footprints_overlap(
            &wedge(Rotation::Deg0),
            &wedge(Rotation::Deg90)
        ));
    }

    #[test]
    fn quantized_single_sided_strip_never_groups_overlapping_halves() {
        // Representative non-square, non-integral quad strip like Blender's
        // single-sided race-track export. Both source triangles require an
        // altitude split after conversion to Brickadia units.
        let data = build(
            &[model(
                &[
                    [0.0, 0.0, 0.0],
                    [1.44, 0.008, 0.0],
                    [1.438_772, -0.007_478, 0.957_871],
                    [0.001_228, -0.091_427, 0.953_436],
                ],
                &[0, 1, 2, 0, 2, 3],
            )],
            &[RgbaImage::from_pixel(1, 1, image::Rgba([255; 4]))],
            &options(),
        )
        .unwrap();

        assert_eq!(data.stats.source_triangles, 2);
        assert_eq!(data.stats.emitted_wedges, 4);
        assert_grids_have_no_overlapping_wedges(&data);
    }

    #[test]
    fn compatible_coplanar_source_faces_share_a_grid() {
        let data = build(
            &[model(
                &[
                    [0.0, 0.0, 0.0],
                    [2.0, 0.0, 0.0],
                    [0.0, 0.0, 2.0],
                    [2.0, 0.0, 2.0],
                ],
                &[0, 1, 2, 3, 2, 1],
            )],
            &[RgbaImage::from_pixel(1, 1, image::Rgba([255; 4]))],
            &options(),
        )
        .unwrap();

        assert_eq!(data.stats.emitted_wedges, 2);
        assert_eq!(data.stats.emitted_grids, 1);
        assert_eq!(data.grids[0].1.len(), 2);
    }

    #[test]
    fn general_triangles_forming_a_coplanar_quad_use_non_overlapping_grids() {
        let data = build(
            &[model(
                &[
                    [0.0, 0.0, 0.0],
                    [2.0, 0.0, 9.0],
                    [10.0, 0.0, 0.0],
                    [8.0, 0.0, -9.0],
                ],
                &[0, 1, 2, 0, 2, 3],
            )],
            &[RgbaImage::from_pixel(1, 1, image::Rgba([255; 4]))],
            &options(),
        )
        .unwrap();

        assert_eq!(data.stats.source_triangles, 2);
        assert_eq!(data.stats.emitted_wedges, 4);
        assert_eq!(data.stats.emitted_grids, 2);
        assert_eq!(
            data.grids
                .iter()
                .map(|(_, bricks)| bricks.len())
                .sum::<usize>(),
            4
        );
        assert_grids_have_no_overlapping_wedges(&data);
    }

    #[test]
    fn wedges_on_different_planes_keep_separate_grids() {
        let data = build(
            &[model(
                &[
                    [0.0, 0.0, 0.0],
                    [2.0, 0.0, 0.0],
                    [0.0, 0.0, 2.0],
                    [0.0, 1.0, 0.0],
                    [2.0, 1.0, 0.0],
                    [0.0, 1.0, 2.0],
                ],
                &[0, 1, 2, 3, 4, 5],
            )],
            &[RgbaImage::from_pixel(1, 1, image::Rgba([255; 4]))],
            &options(),
        )
        .unwrap();

        assert_eq!(data.stats.emitted_wedges, 2);
        assert_eq!(data.stats.emitted_grids, 2);
    }

    #[test]
    fn texture_samples_are_alpha_weighted() {
        let texture = RgbaImage::from_raw(2, 1, vec![255, 0, 255, 0, 20, 100, 40, 255]).unwrap();
        let color = average_image_color(&texture).unwrap();
        assert_eq!([color.r, color.g, color.b], [20, 100, 40]);
    }

    #[test]
    fn texture_sampling_reaches_every_pixel_and_flips_obj_v() {
        let texture = RgbaImage::from_raw(
            2,
            2,
            vec![
                255, 0, 0, 255, 0, 255, 0, 255, // top row
                0, 0, 255, 255, 255, 255, 0, 255, // bottom row
            ],
        )
        .unwrap();

        assert_eq!(
            sample_texture(&texture, Vector2::new(0.75, 0.75)),
            [0, 255, 0, 255]
        );
        assert_eq!(
            sample_texture(&texture, Vector2::new(0.25, 0.25)),
            [0, 0, 255, 255]
        );
    }

    #[test]
    fn fully_transparent_faces_are_not_emitted() {
        let error = build(
            &[model(
                &[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
                &[0, 1, 2],
            )],
            &[RgbaImage::from_pixel(1, 1, image::Rgba([255, 0, 0, 0]))],
            &options(),
        )
        .err()
        .expect("a wholly transparent model should be rejected");
        assert!(error.to_string().contains("no visible"));
    }

    #[test]
    fn load_warning_is_logged_only_above_two_thousand_grids() {
        let at_limit = options();
        log_stats(
            &at_limit,
            &GridMeshStats {
                emitted_grids: 2000,
                ..GridMeshStats::default()
            },
        );
        assert!(!at_limit
            .logger
            .get_messages()
            .iter()
            .any(|message| message.contains("trouble loading")));

        let above_limit = options();
        log_stats(
            &above_limit,
            &GridMeshStats {
                emitted_grids: 2001,
                ..GridMeshStats::default()
            },
        );
        assert!(above_limit
            .logger
            .get_messages()
            .iter()
            .any(|message| message.contains("trouble loading in game")));
    }
}
