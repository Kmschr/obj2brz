use crate::brdb_support;
use crate::error::{ConversionError, ConversionResult, MissingResources};
use crate::ldraw;
use crate::logger::Logger;
use crate::octree;
use crate::rampify;
use crate::fbx;
use crate::gltf_support;
use crate::simplify::*;
use crate::stl;
use crate::voxelize::voxelize;

use brdb::{Brick, Entity};
use cgmath::Vector4;
use serde::{Deserialize, Serialize};
use std::{io::Cursor, path::Path, path::PathBuf};
use tobj::LoadOptions;

const OBJ_ICON: &[u8; 10987] = include_bytes!("../res/obj_icon.png");

/// Intermediate data structure for building the save.
#[derive(Clone)]
pub struct SaveData {
    pub bricks: Vec<Brick>,
    pub author_name: String,
}

/// Pure conversion configuration, decoupled from any UI. Front-ends (CLI, GUI)
/// build this and hand it to [`convert`]. The [`Logger`] doubles as the progress
/// channel: the GUI polls it, the CLI streams it to stdout.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConvertOptions {
    pub bricktype: BrickType,
    pub brick_scale: isize,
    pub input_file_path: String,
    pub material: Material,
    pub material_intensity: u32,
    /// Whether generated bricks block Brickadia players.
    #[serde(default = "default_player_collision")]
    pub player_collision: bool,
    /// Whether generated bricks participate in physics and grid collisions.
    #[serde(default = "default_physics_collision")]
    pub physics_collision: bool,
    pub output_directory: String,
    pub copy_to_clipboard: bool,
    pub output_format: OutputFormat,
    pub save_owner_id: String,
    pub save_owner_name: String,
    pub save_name: String,
    pub scale: f32,
    pub simplify: bool,
    /// Which packer turns voxels into bricks. Applies to both the lossy
    /// (`simplify`) and lossless passes.
    #[serde(default)]
    pub merge_algorithm: MergeAlgorithm,
    /// Flatten each diffuse texture to the colors it actually needs: the
    /// palette size is detected per texture, so a mostly-gray texture ends up
    /// gray while a red texture with yellow stripes keeps both. Fewer distinct
    /// colors let neighboring voxels share a color and merge into larger
    /// bricks, without the hue shift a fixed color grid causes.
    #[serde(default)]
    pub posterize: bool,
    #[serde(default)]
    pub rampify: bool,
    /// Rampify for terrain: only the upward-facing surface is smoothed with
    /// ramps; undersides become plain upright bricks instead of inverted ramps.
    #[serde(default)]
    pub rampify_terrain: bool,
    /// Whether rampify may place corner ramp bricks (outer and inner) where
    /// two perpendicular slopes meet.
    #[serde(default = "default_rampify_corners")]
    pub rampify_corners: bool,
    pub split_by_material: bool,
    pub grid_offset_x: f32,
    pub grid_offset_y: f32,
    pub grid_offset_z: f32,
    #[serde(skip)]
    pub logger: Logger,
}

impl Default for ConvertOptions {
    fn default() -> Self {
        Self {
            bricktype: BrickType::Microbricks,
            brick_scale: 1,
            input_file_path: "test.obj".into(),
            material: Material::Plastic,
            material_intensity: 5,
            player_collision: true,
            physics_collision: true,
            output_directory: "builds".into(),
            copy_to_clipboard: false,
            output_format: OutputFormat::Brz,
            save_owner_id: "d66c4ad5-59fc-4a9b-80b8-08dedc25bff9".into(),
            save_owner_name: "obj2brz".into(),
            save_name: "test".into(),
            scale: 1.0,
            simplify: false,
            merge_algorithm: MergeAlgorithm::default(),
            posterize: false,
            rampify: false,
            rampify_terrain: false,
            rampify_corners: true,
            split_by_material: false,
            grid_offset_x: 0.0,
            grid_offset_y: 0.0,
            grid_offset_z: 0.0,
            logger: Logger::new(),
        }
    }
}

const fn default_player_collision() -> bool {
    true
}

const fn default_rampify_corners() -> bool {
    true
}

const fn default_physics_collision() -> bool {
    true
}

/// How merged bricks are grown out of the voxel grid.
#[derive(Debug, Default, PartialEq, Clone, Copy, Serialize, Deserialize)]
pub enum MergeAlgorithm {
    /// Interior-seeded packer: seeds from the deepest interior of each region
    /// and grows the shortest side first, giving blocky bricks and far fewer
    /// seams on curved shells.
    #[default]
    Squarish,
    /// Morton-order greedy merge: seeds from a region corner and grows z→y→x to
    /// maximum extent. Produces long strips, which suit flat or boxy models.
    Greedy,
}

#[derive(Debug, PartialEq, Clone, Copy, Serialize, Deserialize)]
pub enum BrickType {
    Microbricks,
    Default,
    Tiles,
}

#[derive(Debug, PartialEq, Clone, Copy, Serialize, Deserialize)]
pub enum Material {
    Plastic,
    Glass,
    Glow,
    Metallic,
    Hologram,
    Ghost,
}

#[derive(Debug, PartialEq, Clone, Copy, Serialize, Deserialize)]
pub enum OutputFormat {
    Brz,
    Brdb,
}

/// Axis-aligned bounds of the triangle geometry contained in an OBJ file.
///
/// Coordinates are in the OBJ's own units. Front-ends can combine these
/// dimensions with their selected conversion settings to present an estimated
/// in-game size without voxelizing the entire model first.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelBounds {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl ModelBounds {
    pub fn dimensions(self) -> [f32; 3] {
        [
            self.max[0] - self.min[0],
            self.max[1] - self.min[1],
            self.max[2] - self.min[2],
        ]
    }

    /// Estimates the generated save's width, depth, and height in studs.
    pub fn estimated_stud_dimensions(self, options: &ConvertOptions) -> [f32; 3] {
        // LDraw models are pinned at true scale: the loader measures them in
        // studs and the converter compensates for the voxel grid, so one
        // model unit is always exactly one stud.
        let stud_scale = if ldraw::is_ldraw_path(&options.input_file_path) {
            1.0
        } else {
            options.scale
                * if !options.rampify && options.bricktype == BrickType::Microbricks {
                    options.brick_scale as f32 / 5.0
                } else {
                    1.0
                }
        };
        let [width, height, depth] = self.dimensions();
        [width * stud_scale, depth * stud_scale, height * stud_scale]
    }
}

impl ConvertOptions {
    /// Validates settings that are cheap to check without touching the filesystem.
    pub fn settings_error(&self) -> Option<String> {
        if !self.scale.is_finite() || self.scale <= 0.0 {
            return Some("Scale must be a positive, finite number.".to_string());
        }
        if self.save_name.trim().is_empty()
            || self.save_name.contains(['/', '\\'])
            || self.save_name == "."
            || self.save_name == ".."
        {
            return Some("Save name must be a filename, not a path.".to_string());
        }
        None
    }
}

/// Creates a 1x1 solid color texture from material color
fn create_solid_color_texture(diffuse: [f32; 3], dissolve: f32) -> image::RgbaImage {
    let mut img = image::RgbaImage::new(1, 1);
    img.put_pixel(
        0,
        0,
        image::Rgba([
            float_to_color_channel(diffuse[0]),
            float_to_color_channel(diffuse[1]),
            float_to_color_channel(diffuse[2]),
            float_to_color_channel(dissolve),
        ]),
    );
    img
}

fn float_to_color_channel(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Working palette size handed to NeuQuant before perceptual merging collapses
/// it to however many colors the texture actually needs.
const POSTERIZE_WORKING_COLORS: usize = 64;

/// Oklab distance below which two palette entries read as the same color at
/// cartoon-poster scale. A just-noticeable difference is around 0.02, so this
/// deliberately merges anything short of an obviously different color.
const POSTERIZE_MERGE_DISTANCE: f32 = 0.1;

/// Lightness carries shading, which posterization is meant to flatten, so it
/// counts for less than hue and chroma when deciding if two colors differ.
const POSTERIZE_LIGHTNESS_WEIGHT: f32 = 0.5;

/// One color of the detected palette: the Oklab centroid it competes with, the
/// sRGB value it paints (the dominant member's, not a muddy mean), and how many
/// pixels it owns.
struct PosterizeCluster {
    lab: [f32; 3],
    rgb: [u8; 3],
    weight: u32,
}

fn srgb_to_linear(channel: u8) -> f32 {
    let c = f32::from(channel) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Converts an sRGB triple to Oklab, whose euclidean distance tracks perceived
/// color difference far better than raw RGB does.
fn srgb_to_oklab(rgb: [u8; 3]) -> [f32; 3] {
    let r = srgb_to_linear(rgb[0]);
    let g = srgb_to_linear(rgb[1]);
    let b = srgb_to_linear(rgb[2]);

    let l = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let m = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let s = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();

    [
        0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
    ]
}

fn oklab_distance(a: [f32; 3], b: [f32; 3]) -> f32 {
    let dl = (a[0] - b[0]) * POSTERIZE_LIGHTNESS_WEIGHT;
    let da = a[1] - b[1];
    let db = a[2] - b[2];
    (dl * dl + da * da + db * db).sqrt()
}

/// Repeatedly fuses the closest pair of clusters until every remaining pair is
/// perceptually distinct. The heavier cluster keeps its sRGB value so the
/// result stays a color that actually appears in the texture.
///
/// Linkage is complete, not average: two clusters merge only when *every* color
/// in one is within the threshold of *every* color in the other. Average
/// linkage chains — on a smooth gradient each step is a small hop, so the whole
/// range collapses into a single color and an aerial photo posterizes to two
/// tones. Complete linkage caps each cluster's own spread instead, so gradients
/// break into distinct bands while genuinely flat regions still fuse.
fn merge_similar_clusters(clusters: &mut Vec<PosterizeCluster>) {
    let mut distances: Vec<Vec<f32>> = clusters
        .iter()
        .map(|a| {
            clusters
                .iter()
                .map(|b| oklab_distance(a.lab, b.lab))
                .collect()
        })
        .collect();

    loop {
        let mut closest = None;
        let mut closest_distance = POSTERIZE_MERGE_DISTANCE;
        for a in 0..clusters.len() {
            for b in (a + 1)..clusters.len() {
                if distances[a][b] < closest_distance {
                    closest_distance = distances[a][b];
                    closest = Some((a, b));
                }
            }
        }

        let Some((a, b)) = closest else { return };

        // Complete-linkage update: the fused cluster is as far from every other
        // cluster as its furthest member was.
        for other in 0..clusters.len() {
            let widest = distances[a][other].max(distances[b][other]);
            distances[a][other] = widest;
            distances[other][a] = widest;
        }
        distances[a][a] = 0.0;
        distances.remove(b);
        for row in &mut distances {
            row.remove(b);
        }

        let absorbed = clusters.remove(b);
        let target = &mut clusters[a];
        let total = target.weight + absorbed.weight;
        for axis in 0..3 {
            target.lab[axis] = (target.lab[axis] * target.weight as f32
                + absorbed.lab[axis] * absorbed.weight as f32)
                / total as f32;
        }
        if absorbed.weight > target.weight {
            target.rgb = absorbed.rgb;
        }
        target.weight = total;
    }
}

/// Flattens a texture to the colors it actually needs. A working palette is
/// chosen from the texture's own pixels, then entries that are perceptually
/// indistinguishable are fused: a mostly-gray texture collapses to one or two
/// grays, while red with yellow stripes keeps both. Per-pixel alpha is
/// preserved, and fully transparent pixels are ignored because their RGB is
/// usually undefined garbage that would otherwise steal a palette slot.
/// Returns the number of colors the texture was reduced to.
fn posterize_image(image: &mut image::RgbaImage) -> usize {
    let opaque_pixels: Vec<u8> = image
        .pixels()
        .filter(|pixel| pixel.0[3] > 0)
        .flat_map(|pixel| pixel.0)
        .collect();
    if opaque_pixels.len() / 4 <= POSTERIZE_WORKING_COLORS {
        return distinct_opaque_color_count(image);
    }

    let quantizer = color_quant::NeuQuant::new(3, POSTERIZE_WORKING_COLORS, &opaque_pixels);
    let palette = quantizer.color_map_rgba();
    let entry_count = palette.len() / 4;

    let mut weights = vec![0_u32; entry_count];
    let mut entry_of_pixel = Vec::with_capacity(image.pixels().len());
    for pixel in image.pixels() {
        let entry = if pixel.0[3] > 0 {
            let entry = quantizer.index_of(&pixel.0).min(entry_count - 1);
            weights[entry] += 1;
            Some(entry)
        } else {
            None
        };
        entry_of_pixel.push(entry);
    }

    // Cluster index per surviving palette entry, so pixels can be remapped by
    // table lookup instead of a second nearest-color search.
    let mut cluster_of_entry = vec![0_usize; entry_count];
    let mut clusters = Vec::new();
    for (entry, &weight) in weights.iter().enumerate() {
        if weight == 0 {
            continue;
        }
        let rgb = [palette[entry * 4], palette[entry * 4 + 1], palette[entry * 4 + 2]];
        cluster_of_entry[entry] = clusters.len();
        clusters.push(PosterizeCluster { lab: srgb_to_oklab(rgb), rgb, weight });
    }

    let original_clusters: Vec<[f32; 3]> = clusters.iter().map(|cluster| cluster.lab).collect();
    merge_similar_clusters(&mut clusters);

    // Merging shuffles indices, so re-point each original cluster at whichever
    // survivor now sits closest to it.
    let final_of_original: Vec<usize> = original_clusters
        .iter()
        .map(|lab| {
            clusters
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| {
                    oklab_distance(*lab, a.lab)
                        .total_cmp(&oklab_distance(*lab, b.lab))
                })
                .map(|(index, _)| index)
                .unwrap_or(0)
        })
        .collect();

    for (pixel, entry) in image.pixels_mut().zip(entry_of_pixel) {
        let Some(entry) = entry else { continue };
        let rgb = clusters[final_of_original[cluster_of_entry[entry]]].rgb;
        pixel.0[0] = rgb[0];
        pixel.0[1] = rgb[1];
        pixel.0[2] = rgb[2];
    }

    clusters.len()
}

fn distinct_opaque_color_count(image: &image::RgbaImage) -> usize {
    image
        .pixels()
        .filter(|pixel| pixel.0[3] > 0)
        .map(|pixel| [pixel.0[0], pixel.0[1], pixel.0[2]])
        .collect::<std::collections::HashSet<_>>()
        .len()
}

/// Validates the input model file and checks for missing resources.
/// Dispatches on extension: OBJ files are checked for missing textures, LDraw
/// files for unresolved part references; other formats validate trivially.
pub fn validate_obj_resources(obj_path: &str) -> ConversionResult<MissingResources> {
    let p = Path::new(obj_path);

    // Check if the input file exists
    if !p.exists() {
        return Err(ConversionError::ObjFileNotFound { path: p.to_path_buf() });
    }

    if stl::is_stl_path(obj_path)
        || fbx::is_fbx_path(obj_path)
        || gltf_support::is_gltf_path(obj_path)
    {
        return Ok(MissingResources::new());
    }

    if ldraw::is_ldraw_path(obj_path) {
        let loaded = ldraw::load_ldraw(p, &Logger::new())?;
        let mut missing = MissingResources::new();
        missing.missing_subfiles = loaded.missing_subfiles;
        return Ok(missing);
    }

    let load_options = LoadOptions {
        triangulate: true,
        ignore_lines: true,
        ignore_points: true,
        single_index: true,
    };

    let (_models, materials) = tobj::load_obj(obj_path, &load_options)
        .map_err(|e| ConversionError::ObjParseError(e.to_string()))?;

    let mut missing = MissingResources::new();

    // Materials are optional in OBJ. The converter deliberately supports
    // untextured meshes with a white fallback, so only report missing texture
    // files for materials that actually reference one.
    let materials = match materials {
        Ok(mats) if !mats.is_empty() => mats,
        Ok(_) | Err(_) => return Ok(missing),
    };

    // Check each material for missing textures
    for material in materials {
        if let Some(texture_name) = &material.diffuse_texture {
            if !texture_name.is_empty() {
                let texture_path = p.parent()
                    .ok_or_else(|| ConversionError::ObjFileNotFound { path: p.to_path_buf() })?
                    .join(texture_name);

                if !texture_path.exists() {
                    missing.missing_textures.push((material.name.clone(), texture_path));
                }
            }
        }
    }

    Ok(missing)
}

/// Reads an OBJ's triangle geometry and returns its axis-aligned bounds.
///
/// This intentionally does not load textures, keeping size estimates fast and
/// independent of whether the model's material resources are available.
fn default_load_options() -> LoadOptions {
    LoadOptions {
        triangulate: true,
        ignore_lines: true,
        ignore_points: true,
        single_index: true,
    }
}

/// Material loader used when an OBJ is parsed from bytes: the sibling `.mtl`
/// isn't available in the browser, so no materials are resolved.
fn no_materials(_: &Path) -> tobj::MTLLoadResult {
    Ok((Vec::new(), Default::default()))
}

pub fn model_bounds(obj_path: &str) -> ConversionResult<ModelBounds> {
    let path = Path::new(obj_path);
    if !path.exists() {
        return Err(ConversionError::ObjFileNotFound {
            path: path.to_path_buf(),
        });
    }

    if stl::is_stl_path(obj_path) {
        return bounds_from_models(&stl::load_stl(path)?);
    }

    if fbx::is_fbx_path(obj_path) {
        return bounds_from_models(&fbx::load_fbx(path)?.0);
    }

    if gltf_support::is_gltf_path(obj_path) {
        return bounds_from_models(&gltf_support::load_gltf(path)?.0);
    }

    if ldraw::is_ldraw_path(obj_path) {
        let loaded = ldraw::load_ldraw(path, &Logger::new())?;
        return bounds_from_models(&loaded.models);
    }

    let (models, _) = tobj::load_obj(obj_path, &default_load_options())
        .map_err(|e| ConversionError::ObjParseError(e.to_string()))?;

    bounds_from_models(&models)
}

/// Measures triangle bounds from an in-memory model (format detected from
/// content). Used by the browser build, which has no filesystem path to read.
pub fn model_bounds_from_bytes(obj_bytes: &[u8]) -> ConversionResult<ModelBounds> {
    if stl::looks_like_stl(obj_bytes) {
        return bounds_from_models(&stl::load_stl_bytes(obj_bytes)?);
    }

    if fbx::looks_like_fbx(obj_bytes) {
        return bounds_from_models(&fbx::load_fbx_bytes(obj_bytes)?.0);
    }

    if gltf_support::looks_like_gltf(obj_bytes) {
        return bounds_from_models(&gltf_support::load_gltf_bytes(obj_bytes)?.0);
    }

    if ldraw::looks_like_ldraw(obj_bytes) {
        let loaded = ldraw::load_ldraw_bytes(obj_bytes, &Logger::new())?;
        return bounds_from_models(&loaded.models);
    }

    let mut reader = Cursor::new(obj_bytes);
    let (models, _) = tobj::load_obj_buf(&mut reader, &default_load_options(), no_materials)
        .map_err(|e| ConversionError::ObjParseError(e.to_string()))?;

    bounds_from_models(&models)
}

fn bounds_from_models(models: &[tobj::Model]) -> ConversionResult<ModelBounds> {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    let mut has_triangle = false;

    for model in models {
        let mesh = &model.mesh;
        for indices in mesh.indices.chunks_exact(3) {
            let vertex = |index: u32| {
                let offset = index as usize * 3;
                mesh.positions.get(offset..offset + 3)
            };
            let (Some(a), Some(b), Some(c)) =
                (vertex(indices[0]), vertex(indices[1]), vertex(indices[2]))
            else {
                continue;
            };

            has_triangle = true;
            for position in [a, b, c] {
                for axis in 0..3 {
                    min[axis] = min[axis].min(position[axis]);
                    max[axis] = max[axis].max(position[axis]);
                }
            }
        }
    }

    if !has_triangle {
        return Err(ConversionError::ObjParseError(
            "OBJ contains no triangle geometry to measure".to_string(),
        ));
    }

    Ok(ModelBounds { min, max })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unit cube as an OBJ, enough volume for the voxelizer to produce bricks.
    const CUBE_OBJ: &str = "\
v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nv 0 0 1\nv 1 0 1\nv 1 1 1\nv 0 1 1\n\
f 1 2 3\nf 1 3 4\nf 5 6 7\nf 5 7 8\nf 1 2 6\nf 1 6 5\n\
f 2 3 7\nf 2 7 6\nf 3 4 8\nf 3 8 7\nf 4 1 5\nf 4 5 8\n";

    #[test]
    fn converts_in_memory_obj_to_brz_bytes() {
        let opts = ConvertOptions {
            scale: 8.0,
            ..ConvertOptions::default()
        };

        let bytes = convert_obj_bytes_to_brz(&opts, CUBE_OBJ.as_bytes()).unwrap();

        assert!(bytes.starts_with(b"BRZ"), "output is not a BRZ archive");
    }

    #[test]
    fn measures_bounds_from_bytes() {
        let bounds = model_bounds_from_bytes(CUBE_OBJ.as_bytes()).unwrap();
        assert_eq!(bounds.min, [0.0, 0.0, 0.0]);
        assert_eq!(bounds.max, [1.0, 1.0, 1.0]);
    }

    /// 40x40x24 LDU red box (a 2x2 brick) out of LDraw quads.
    const BOX_DAT: &str = "\
0 ldraw test box
4 4 -20 -24 -20 20 -24 -20 20 -24 20 -20 -24 20
4 4 -20 0 -20 20 0 -20 20 0 20 -20 0 20
4 4 -20 -24 -20 20 -24 -20 20 0 -20 -20 0 -20
4 4 -20 -24 20 20 -24 20 20 0 20 -20 0 20
4 4 -20 -24 -20 -20 -24 20 -20 0 20 -20 0 -20
4 4 20 -24 -20 20 -24 20 20 0 20 20 0 -20
";

    #[test]
    fn converts_in_memory_ldraw_to_brz_bytes() {
        let opts = ConvertOptions {
            scale: 4.0,
            ..ConvertOptions::default()
        };

        let bytes = convert_obj_bytes_to_brz(&opts, BOX_DAT.as_bytes()).unwrap();

        assert!(bytes.starts_with(b"BRZ"), "output is not a BRZ archive");
    }

    #[test]
    fn measures_ldraw_bounds_from_bytes() {
        let bounds = model_bounds_from_bytes(BOX_DAT.as_bytes()).unwrap();
        assert_eq!(bounds.min, [-1.0, 0.0, -1.0]);
        assert_eq!(bounds.max, [1.0, 1.2, 1.0]);
    }

    /// Converts BOX_DAT and measures the generated bricks' bounding box in
    /// Brickadia world units (brick sizes are half-extents).
    fn converted_ldraw_box_world_dimensions(opts: &ConvertOptions) -> [i32; 3] {
        let (mut models, images) = load_models_from_buf(BOX_DAT.as_bytes(), opts).unwrap();
        let mut octree = voxelize_models(&mut models, &images, opts, None);
        let save_data = octree_to_save_data(&mut octree, opts, None).unwrap();
        assert!(!save_data.bricks.is_empty());

        let mut min = [i32::MAX; 3];
        let mut max = [i32::MIN; 3];
        for brick in &save_data.bricks {
            let brdb::BrickType::Procedural { size, .. } = &brick.asset else {
                panic!("expected procedural bricks");
            };
            let position = [brick.position.x, brick.position.y, brick.position.z];
            let size = [size.x as i32, size.y as i32, size.z as i32];
            for axis in 0..3 {
                min[axis] = min[axis].min(position[axis] - size[axis]);
                max[axis] = max[axis].max(position[axis] + size[axis]);
            }
        }
        [max[0] - min[0], max[1] - min[1], max[2] - min[2]]
    }

    #[test]
    fn ldraw_bricks_span_true_world_size() {
        // A 2x2 LEGO brick must span 2x2 studs (20x20 units) and one brick
        // height (12 units) in Brickadia, whatever the scale setting says.
        // Exactness needs the voxel size to divide both 10 (stud) and 12
        // (brick height): 1x microbricks (2-unit voxels) and default bricks
        // (10x10x4 voxels) qualify; larger microbrick scales round to the
        // coarser grid.
        for (bricktype, brick_scale, scale) in [
            (BrickType::Microbricks, 1, 3.0),
            (BrickType::Microbricks, 1, 0.5),
            (BrickType::Default, 1, 10.0),
        ] {
            let opts = ConvertOptions {
                input_file_path: "brick.dat".into(),
                bricktype,
                brick_scale,
                scale,
                ..ConvertOptions::default()
            };
            assert_eq!(
                converted_ldraw_box_world_dimensions(&opts),
                [20, 20, 12],
                "wrong world size for {bricktype:?} brick_scale={brick_scale} scale={scale}"
            );
        }
    }

    #[test]
    fn ldraw_models_are_pinned_at_true_scale() {
        let bounds = model_bounds_from_bytes(BOX_DAT.as_bytes()).unwrap();

        // A 2x2 LEGO brick is 2x2 studs and 1.2 studs tall, regardless of the
        // scale setting or brick type.
        for (bricktype, brick_scale, scale) in [
            (BrickType::Microbricks, 1, 3.0),
            (BrickType::Microbricks, 5, 0.5),
            (BrickType::Default, 1, 10.0),
        ] {
            let opts = ConvertOptions {
                input_file_path: "brick.dat".into(),
                bricktype,
                brick_scale,
                scale,
                ..ConvertOptions::default()
            };
            assert_eq!(bounds.estimated_stud_dimensions(&opts), [2.0, 2.0, 1.2]);

            // One stud must span exactly 10 world units on the voxel grid:
            // microbrick voxels are 2 * brick_scale units, others 10 units.
            let voxel_units = match bricktype {
                BrickType::Microbricks => 2 * brick_scale,
                _ => 10,
            };
            assert_eq!(ldraw_scale(&opts) * voxel_units as f32, 10.0);
        }
    }

    #[test]
    fn measures_ldraw_bounds_from_file() {
        let path = std::env::temp_dir().join(format!(
            "obj2brz-ldraw-{}-{}.dat",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        std::fs::write(&path, BOX_DAT).unwrap();

        let bounds = model_bounds(path.to_str().unwrap()).unwrap();
        assert_eq!(bounds.min, [-1.0, 0.0, -1.0]);
        assert_eq!(bounds.max, [1.0, 1.2, 1.0]);

        let missing = validate_obj_resources(path.to_str().unwrap()).unwrap();
        assert!(!missing.has_issues());

        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn measures_only_triangle_geometry() {
        let path = std::env::temp_dir().join(format!(
            "obj2brz-bounds-{}-{}.obj",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        std::fs::write(
            &path,
            "v -2 4 10\nv 3 9 -1\nv 1 6 5\nv 999 999 999\nf 1 2 3\n",
        )
        .unwrap();

        let bounds = model_bounds(path.to_str().unwrap()).unwrap();
        assert_eq!(bounds.min, [-2.0, 4.0, -1.0]);
        assert_eq!(bounds.max, [3.0, 9.0, 10.0]);
        assert_eq!(bounds.dimensions(), [5.0, 5.0, 11.0]);

        let default_options = ConvertOptions {
            bricktype: BrickType::Default,
            scale: 2.0,
            ..ConvertOptions::default()
        };
        assert_eq!(bounds.estimated_stud_dimensions(&default_options), [10.0, 22.0, 10.0]);

        let micro_options = ConvertOptions {
            brick_scale: 2,
            ..ConvertOptions::default()
        };
        assert_eq!(bounds.estimated_stud_dimensions(&micro_options), [2.0, 4.4, 2.0]);

        std::fs::remove_file(path).unwrap();
    }

    /// Builds a texture by repeating `colors` across the pixels, large enough
    /// that posterization does not bail out on a tiny image.
    fn texture_of(colors: &[[u8; 4]]) -> image::RgbaImage {
        image::RgbaImage::from_fn(64, 64, |x, y| {
            image::Rgba(colors[((x + y * 64) as usize) % colors.len()])
        })
    }

    fn distinct_opaque_colors(image: &image::RgbaImage) -> std::collections::HashSet<[u8; 3]> {
        image
            .pixels()
            .filter(|pixel| pixel.0[3] > 0)
            .map(|pixel| [pixel.0[0], pixel.0[1], pixel.0[2]])
            .collect()
    }

    #[test]
    fn posterize_collapses_near_identical_shades_to_one_color() {
        let mut image = texture_of(&[
            [128, 128, 128, 255],
            [130, 130, 130, 255],
            [126, 127, 129, 255],
            [131, 129, 128, 255],
        ]);

        posterize_image(&mut image);

        assert_eq!(distinct_opaque_colors(&image).len(), 1);
    }

    #[test]
    fn posterize_keeps_visibly_different_colors_apart() {
        let mut image = texture_of(&[[200, 20, 20, 255], [230, 220, 40, 255]]);

        posterize_image(&mut image);

        let colors = distinct_opaque_colors(&image);
        assert_eq!(colors.len(), 2);
        // Both survivors should still read as red and yellow, not a blend.
        assert!(colors.iter().any(|c| c[0] > c[2] && c[1] < 100));
        assert!(colors.iter().any(|c| c[0] > 150 && c[1] > 150));
    }

    #[test]
    fn posterize_ignores_transparent_pixels() {
        let mut image = texture_of(&[[10, 200, 10, 255], [255, 0, 255, 0]]);

        posterize_image(&mut image);

        assert_eq!(distinct_opaque_colors(&image).len(), 1);
        let transparent = image
            .pixels()
            .find(|pixel| pixel.0[3] == 0)
            .expect("transparent pixels should be preserved");
        assert_eq!(transparent.0[3], 0);
    }
}

/// Runs a full conversion described by `opts`, writing the resulting save to disk.
pub fn convert(opts: &ConvertOptions, skip_textures: bool) -> ConversionResult<()> {
    if opts.split_by_material {
        // Load models and materials once
        opts.logger.log("Loading models and materials...".to_string());
        let (mut models, material_images) = load_models_and_materials(opts, skip_textures)?;
        let material_count = models
            .iter()
            .filter_map(|model| model.mesh.material_id)
            .max()
            .map_or(0, |id| id + 1);

        if material_count == 0 {
            opts.logger.log("No material assignments found, using a single grid".to_string());
            let mut octree = voxelize_models(&mut models, &material_images, opts, None);
            return write_brz_data(&mut octree, opts, None);
        }

        opts.logger.log(format!("Found {} materials, processing each separately", material_count));

        // Process each material separately
        let mut material_grids: Vec<(Entity, Vec<Brick>)> = Vec::new();

        for mat_id in 0..material_count {
            opts.logger.log(format!("Processing material {} of {}", mat_id + 1, material_count));

            // Voxelize only this material
            let mut octree = voxelize_models(&mut models, &material_images, opts, Some(mat_id));

            let max_merge = 500;
            let mut save_data = SaveData {
                bricks: Vec::new(),
                author_name: opts.save_owner_name.clone(),
            };

            opts.logger.log(format!("Processing material {}...", mat_id));
            if opts.rampify {
                rampify::rampify(&octree, &mut save_data, opts)?;
            } else if opts.merge_algorithm == MergeAlgorithm::Squarish {
                simplify_squarish(&octree, &mut save_data, opts, max_merge, opts.simplify);
            } else if opts.simplify {
                simplify_lossy(&mut octree, &mut save_data, opts, max_merge);
            } else {
                simplify_lossless(&mut octree, &mut save_data, opts, max_merge);
            }

            if !save_data.bricks.is_empty() {
                opts.logger.log(format!("Material {} generated {} bricks", mat_id, save_data.bricks.len()));

                // Create a frozen grid entity for this material with user-defined offset
                let offset_multiplier = mat_id as f32;
                let entity = Entity {
                    frozen: true,
                    location: brdb::Vector3f {
                        x: opts.grid_offset_x * offset_multiplier,
                        y: opts.grid_offset_y * offset_multiplier,
                        z: opts.grid_offset_z * offset_multiplier,
                    },
                    ..Default::default()
                };

                material_grids.push((entity, save_data.bricks));
            } else {
                opts.logger.log(format!("Material {} had no bricks, skipping", mat_id));
            }
        }

        write_brz_with_grids(opts, material_grids)
    } else {
        // Regular single-grid conversion
        let mut octree = generate_octree(opts, skip_textures, None)?;
        write_brz_data(&mut octree, opts, None)
    }
}

fn load_models_and_materials(
    opt: &ConvertOptions,
    skip_textures: bool,
) -> ConversionResult<(Vec<tobj::Model>, Vec<image::RgbaImage>)> {
    let p = Path::new(&opt.input_file_path);

    if stl::is_stl_path(&opt.input_file_path) {
        opt.logger.log("Importing STL model...".to_string());
        return finish_untextured_models(stl::load_stl(p)?, opt);
    }

    if fbx::is_fbx_path(&opt.input_file_path) {
        opt.logger.log("Importing FBX model...".to_string());
        let (models, material_images) = fbx::load_fbx(p)?;
        return finish_prebaked_models(models, material_images, opt);
    }

    if gltf_support::is_gltf_path(&opt.input_file_path) {
        opt.logger.log("Importing glTF model...".to_string());
        let (models, material_images) = gltf_support::load_gltf(p)?;
        return finish_prebaked_models(models, material_images, opt);
    }

    if ldraw::is_ldraw_path(&opt.input_file_path) {
        return load_ldraw_models(ldraw::load_ldraw(p, &opt.logger)?, opt);
    }

    opt.logger.log("Importing model...".to_string());
    let load_options = LoadOptions {
        triangulate: true,
        ignore_lines: true,
        ignore_points: true,
        single_index: true,
    };
    let (mut models, materials) = tobj::load_obj(&opt.input_file_path, &load_options)
        .map_err(|e| ConversionError::ObjParseError(e.to_string()))?;

    if !models.iter().any(|model| {
        model.mesh.indices.len() >= 3 && model.mesh.positions.len() >= 3
    }) {
        return Err(ConversionError::ObjParseError(
            "OBJ contains no triangle geometry to voxelize".to_string(),
        ));
    }

    opt.logger.log("Loading materials...".to_string());
    let mut material_images = Vec::<image::RgbaImage>::new();

    let materials = materials.unwrap_or_else(|_| Vec::new());

    if materials.is_empty() {
        opt.logger.log("  No materials found, using default white color".to_string());
        material_images.push(create_solid_color_texture([1.0, 1.0, 1.0], 1.0));
    } else {
        for material in materials {
            // Try to load texture if available and not skipping
            if !skip_textures {
                if let Some(ref texture_name) = material.diffuse_texture {
                    if texture_name.is_empty() {
                        // Empty texture name, use material color
                        let diffuse = material.diffuse.unwrap_or([1.0, 1.0, 1.0]);
                        let dissolve = material.dissolve.unwrap_or(1.0);
                        material_images.push(create_solid_color_texture(diffuse, dissolve));
                        continue;
                    }
                    let image_path = p.parent()
                        .ok_or_else(|| ConversionError::ObjFileNotFound { path: p.to_path_buf() })?
                        .join(texture_name);

                    opt.logger.log(format!(
                        "  Loading diffuse texture for {} from: {:?}",
                        material.name, image_path
                    ));

                    // Try to load texture
                    match image::open(&image_path) {
                        Ok(img) => {
                            material_images.push(img.into_rgba8());
                        }
                        Err(e) => {
                            return Err(ConversionError::TextureLoadError {
                                path: image_path,
                                reason: e.to_string(),
                            });
                        }
                    }
                } else {
                    // No texture or empty texture name
                    opt.logger.log(format!(
                        "  Material {} does not have a texture, using material color",
                        material.name
                    ));
                    let diffuse = material.diffuse.unwrap_or([1.0, 1.0, 1.0]);
                    let dissolve = material.dissolve.unwrap_or(1.0);
                    material_images.push(create_solid_color_texture(diffuse, dissolve));
                }
            } else {
                // Skipping textures, use material color
                opt.logger.log(format!(
                    "  Skipping textures for material {}, using material color",
                    material.name
                ));
                let diffuse = material.diffuse.unwrap_or([1.0, 1.0, 1.0]);
                let dissolve = material.dissolve.unwrap_or(1.0);
                material_images.push(create_solid_color_texture(diffuse, dissolve));
            }
        }
    }

    if opt.posterize {
        for (index, image) in material_images.iter_mut().enumerate() {
            let colors = posterize_image(image);
            opt.logger
                .log(format!("  Posterized texture {index} to {colors} colors"));
        }
    }

    // Scale models
    scale_models(
        &mut models,
        opt.scale,
        if opt.rampify { BrickType::Default } else { opt.bricktype },
    );

    Ok((models, material_images))
}

/// Finishes a load from a format that carries no materials (e.g. STL):
/// pairs the geometry with the default white material and applies the same
/// scaling pass the OBJ path uses.
fn finish_untextured_models(
    models: Vec<tobj::Model>,
    opt: &ConvertOptions,
) -> ConversionResult<(Vec<tobj::Model>, Vec<image::RgbaImage>)> {
    let material_images = vec![create_solid_color_texture([1.0, 1.0, 1.0], 1.0)];
    finish_prebaked_models(models, material_images, opt)
}

/// Finishes a load from a format whose loader already produced material
/// images (e.g. FBX): validates the geometry and applies the same scaling
/// pass the OBJ path uses.
fn finish_prebaked_models(
    mut models: Vec<tobj::Model>,
    material_images: Vec<image::RgbaImage>,
    opt: &ConvertOptions,
) -> ConversionResult<(Vec<tobj::Model>, Vec<image::RgbaImage>)> {
    if !models
        .iter()
        .any(|model| model.mesh.indices.len() >= 3 && model.mesh.positions.len() >= 3)
    {
        return Err(ConversionError::ObjParseError(
            "model contains no triangle geometry to voxelize".to_string(),
        ));
    }

    scale_models(
        &mut models,
        opt.scale,
        if opt.rampify { BrickType::Default } else { opt.bricktype },
    );

    Ok((models, material_images))
}

/// Voxel-grid scale that pins LDraw models at true size: one LEGO stud (20
/// LDU, one loader unit) = one Brickadia stud (10 world units). Default
/// bricks and ramps voxelize at one stud per voxel, so no scaling is needed;
/// microbricks voxelize at `2 * brick_scale` world units per voxel and need
/// `5 / brick_scale` voxels per stud. The LEGO 24-LDU brick height lands
/// exactly on Brickadia's 12-unit brick height in both cases.
fn ldraw_scale(opts: &ConvertOptions) -> f32 {
    if !opts.rampify && opts.bricktype == BrickType::Microbricks {
        5.0 / opts.brick_scale.max(1) as f32
    } else {
        1.0
    }
}

/// Finishes an LDraw load: reports unresolved parts and applies the same
/// scaling pass the OBJ path uses, at the fixed true-scale factor — the
/// user scale setting deliberately does not apply to LDraw models. LDraw
/// colours arrive pre-baked as one solid-colour material per model, so
/// textures are never involved.
fn load_ldraw_models(
    loaded: ldraw::LDrawModel,
    opt: &ConvertOptions,
) -> ConversionResult<(Vec<tobj::Model>, Vec<image::RgbaImage>)> {
    if !loaded.missing_subfiles.is_empty() {
        opt.logger.log(format!(
            "Warning: {} unresolved LDraw part(s); their geometry is skipped",
            loaded.missing_subfiles.len()
        ));
    }

    if opt.scale != 1.0 {
        opt.logger.log(
            "LDraw models convert at true scale (1 LEGO stud = 1 Brickadia stud); scale setting ignored"
                .to_string(),
        );
    }

    let mut models = loaded.models;
    nudge_off_voxel_boundaries(&mut models);
    scale_models(
        &mut models,
        ldraw_scale(opt),
        if opt.rampify { BrickType::Default } else { opt.bricktype },
    );

    Ok((models, loaded.material_images))
}

/// LDraw geometry is grid-aligned: every face lies exactly on a voxel
/// boundary, where the voxelizer claims the voxels on both sides and fattens
/// each part by one voxel per side. Shrinking the model by an epsilon about
/// its center pulls every exterior face strictly inside its own voxel (faces
/// near the center barely move, but those only produce interior voxels that
/// are filled anyway). The model is then rested just above the ground plane:
/// this keeps the bottom face off the zero boundary, and `scale_models` only
/// re-grounds meshes that dip below zero, so the margin survives.
fn nudge_off_voxel_boundaries(models: &mut [tobj::Model]) {
    const EPSILON: f32 = 1e-4;

    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for model in models.iter() {
        for vertex in model.mesh.positions.chunks_exact(3) {
            for axis in 0..3 {
                min[axis] = min[axis].min(vertex[axis]);
                max[axis] = max[axis].max(vertex[axis]);
            }
        }
    }
    if min.iter().any(|value| !value.is_finite()) {
        return;
    }

    let center = [
        (min[0] + max[0]) / 2.0,
        (min[1] + max[1]) / 2.0,
        (min[2] + max[2]) / 2.0,
    ];
    // The shrunken bottom face sits at half the margin the shrink gave the
    // top face, so both stay strictly inside their voxels.
    let shrunk_min_z = center[2] + (min[2] - center[2]) * (1.0 - EPSILON);
    let lift = 0.25 * EPSILON * (max[2] - min[2]).max(1.0) - shrunk_min_z;

    for model in models.iter_mut() {
        for vertex in model.mesh.positions.chunks_exact_mut(3) {
            for axis in 0..3 {
                vertex[axis] = center[axis] + (vertex[axis] - center[axis]) * (1.0 - EPSILON);
            }
            vertex[2] += lift;
        }
    }
}

fn scale_models(models: &mut [tobj::Model], scale: f32, bricktype: BrickType) {
    // Determine model AABB to expand triangle octree to final size
    // Multiply y-coordinate by 2.5 to take into account plates
    let yscale = if bricktype == BrickType::Microbricks { 1.0 } else { 2.5 };

    for m in models.iter_mut() {
        let p = &mut m.mesh.positions;
        for v in (0..p.len()).step_by(3) {
            p[v] *= scale;
            p[v + 1] *= yscale * scale;
            p[v + 2] *= scale;
        }
    }

    // Raise mesh so no vertices are vertically negative
    if let Some(first_model) = models.first() {
        let positions = &first_model.mesh.positions;
        if !positions.is_empty() {
            let mut min_z = positions[2];
            for m in models.iter() {
                let p = &m.mesh.positions;
                for v in (0..p.len()).step_by(3) {
                    min_z = min_z.min(p[v + 2]);
                }
            }

            if min_z < 0.0 {
                let z_offset = -min_z;
                for m in models.iter_mut() {
                    let p = &mut m.mesh.positions;
                    for v in (0..p.len()).step_by(3) {
                        p[v + 2] += z_offset;
                    }
                }
            }
        }
    }
}

fn voxelize_models(
    models: &mut [tobj::Model],
    material_images: &[image::RgbaImage],
    opts: &ConvertOptions,
    material_filter: Option<usize>,
) -> octree::VoxelTree<Vector4<u8>> {
    if let Some(filter_id) = material_filter {
        opts.logger.log(format!("Voxelizing material {}...", filter_id));
    } else {
        opts.logger.log("Voxelizing...".to_string());
    }
    voxelize(
        models,
        material_images,
        opts.scale,
        opts.bricktype,
        material_filter,
    )
}

fn generate_octree(opt: &ConvertOptions, skip_textures: bool, material_filter: Option<usize>) -> ConversionResult<octree::VoxelTree<Vector4<u8>>> {
    opt.logger.log(format!("Loading {:?}", Path::new(&opt.input_file_path)));
    let (mut models, material_images) = load_models_and_materials(opt, skip_textures)?;
    Ok(voxelize_models(&mut models, &material_images, opt, material_filter))
}

/// Loads model geometry straight from bytes for the browser build; the format
/// is detected from the content. Without the sibling `.mtl`/textures every OBJ
/// face falls back to a default white material; LDraw colours are
/// self-contained and survive intact.
fn load_models_from_buf(
    obj_bytes: &[u8],
    opt: &ConvertOptions,
) -> ConversionResult<(Vec<tobj::Model>, Vec<image::RgbaImage>)> {
    if stl::looks_like_stl(obj_bytes) {
        opt.logger.log("Importing STL model...".to_string());
        return finish_untextured_models(stl::load_stl_bytes(obj_bytes)?, opt);
    }

    if fbx::looks_like_fbx(obj_bytes) {
        opt.logger.log("Importing FBX model...".to_string());
        let (models, material_images) = fbx::load_fbx_bytes(obj_bytes)?;
        return finish_prebaked_models(models, material_images, opt);
    }

    if gltf_support::looks_like_gltf(obj_bytes) {
        opt.logger.log("Importing glTF model...".to_string());
        let (models, material_images) = gltf_support::load_gltf_bytes(obj_bytes)?;
        return finish_prebaked_models(models, material_images, opt);
    }

    if ldraw::looks_like_ldraw(obj_bytes) {
        return load_ldraw_models(ldraw::load_ldraw_bytes(obj_bytes, &opt.logger)?, opt);
    }

    let mut reader = Cursor::new(obj_bytes);
    let (mut models, _materials) =
        tobj::load_obj_buf(&mut reader, &default_load_options(), no_materials)
            .map_err(|e| ConversionError::ObjParseError(e.to_string()))?;

    if !models
        .iter()
        .any(|model| model.mesh.indices.len() >= 3 && model.mesh.positions.len() >= 3)
    {
        return Err(ConversionError::ObjParseError(
            "OBJ contains no triangle geometry to voxelize".to_string(),
        ));
    }

    let material_images = vec![create_solid_color_texture([1.0, 1.0, 1.0], 1.0)];
    scale_models(
        &mut models,
        opt.scale,
        if opt.rampify { BrickType::Default } else { opt.bricktype },
    );

    Ok((models, material_images))
}

/// Converts an in-memory OBJ into BRZ bytes. This is the browser entry point:
/// input arrives as bytes (no filesystem) and the encoded save is returned for
/// the caller to hand to a download.
pub fn convert_obj_bytes_to_brz(opts: &ConvertOptions, obj_bytes: &[u8]) -> ConversionResult<Vec<u8>> {
    opts.logger.log("Importing model...".to_string());
    let (mut models, material_images) = load_models_from_buf(obj_bytes, opts)?;
    let mut octree = voxelize_models(&mut models, &material_images, opts, None);
    let save_data = octree_to_save_data(&mut octree, opts, None)?;
    opts.logger.log(format!("Writing {} bricks...", save_data.bricks.len()));
    let preview = obj_preview_jpg()?;
    brdb_support::brz_bytes(&opts.save_name, &save_data, opts, Some(preview))
}

/// Runs the simplify/rampify pass, turning a voxel octree into brick save data.
fn octree_to_save_data(
    octree: &mut octree::VoxelTree<Vector4<u8>>,
    opts: &ConvertOptions,
    material_id: Option<usize>,
) -> ConversionResult<SaveData> {
    let max_merge = 500;

    let mut save_data = SaveData {
        bricks: Vec::new(),
        author_name: opts.save_owner_name.clone(),
    };

    if let Some(id) = material_id {
        opts.logger.log(format!("Processing material {}...", id));
    } else {
        opts.logger.log(if opts.rampify { "Rampifying..." } else { "Simplifying..." }.to_string());
    }

    if opts.rampify {
        rampify::rampify(octree, &mut save_data, opts)?;
    } else if opts.merge_algorithm == MergeAlgorithm::Squarish {
        simplify_squarish(octree, &mut save_data, opts, max_merge, opts.simplify);
    } else if opts.simplify {
        simplify_lossy(octree, &mut save_data, opts, max_merge);
    } else {
        simplify_lossless(octree, &mut save_data, opts, max_merge);
    }

    Ok(save_data)
}

/// Renders the bundled obj2brz icon to a JPEG for use as the save preview.
fn obj_preview_jpg() -> ConversionResult<Vec<u8>> {
    let preview = image::load_from_memory_with_format(OBJ_ICON, image::ImageFormat::Png)
        .map_err(|e| ConversionError::SaveWriteError(format!("Failed to load preview icon: {}", e)))?;

    let mut preview_bytes_jpg = Vec::new();
    preview
        .write_to(&mut Cursor::new(&mut preview_bytes_jpg), image::ImageOutputFormat::Jpeg(85))
        .map_err(|e| ConversionError::SaveWriteError(format!("Failed to encode JPEG preview: {}", e)))?;

    Ok(preview_bytes_jpg)
}

fn write_brz_data(octree: &mut octree::VoxelTree<Vector4<u8>>, opts: &ConvertOptions, material_id: Option<usize>) -> ConversionResult<()> {
    // A model too large for one rampify grid comes back as several chunks,
    // each of which becomes its own frozen grid.
    if opts.rampify {
        opts.logger.log("Rampifying...".to_string());
        let chunks = rampify::rampify_chunked(octree, opts)?;
        if chunks.len() > 1 {
            let grids = chunks
                .into_iter()
                .map(|bricks| (frozen_grid_entity(), bricks))
                .collect();
            return write_brz_with_grids(opts, grids);
        }

        let save_data = SaveData {
            bricks: chunks.into_iter().next().unwrap_or_default(),
            author_name: opts.save_owner_name.clone(),
        };
        return write_save_data(opts, &save_data);
    }

    let save_data = octree_to_save_data(octree, opts, material_id)?;
    write_save_data(opts, &save_data)
}

/// A frozen grid pinned at the world origin. Chunk bricks keep their absolute
/// positions, so every chunk grid must sit at the same spot to line up.
fn frozen_grid_entity() -> Entity {
    Entity {
        frozen: true,
        ..Default::default()
    }
}

fn write_save_data(opts: &ConvertOptions, save_data: &SaveData) -> ConversionResult<()> {
    // Write file
    opts.logger.log(format!("Writing {} bricks...", save_data.bricks.len()));

    let preview = obj_preview_jpg()?;
    let output_file_path = output_file_path(opts);

    // Determine if we should use procedural bricks based on brick type
    let use_procedural = opts.rampify || opts.bricktype != BrickType::Default;

    brdb_support::write_brz(
        output_file_path.clone(),
        &save_data,
        opts,
        use_procedural,
        Some(preview),
    )?;

    opts.logger.log(format!("Save written to: {:?}", output_file_path));
    Ok(())
}

fn write_brz_with_grids(opts: &ConvertOptions, grids: Vec<(Entity, Vec<Brick>)>) -> ConversionResult<()> {
    opts.logger.log(format!("Writing {} frozen grids...", grids.len()));

    let preview = obj_preview_jpg()?;
    let output_file_path = output_file_path(opts);

    brdb_support::write_brz_grids(
        output_file_path.clone(),
        grids,
        opts,
        Some(preview),
    )?;

    opts.logger.log(format!("Save written to: {:?}", output_file_path));
    Ok(())
}

/// Resolves the output file path (directory + save name + format extension).
pub fn output_file_path(opts: &ConvertOptions) -> PathBuf {
    let extension = match opts.output_format {
        OutputFormat::Brz => "brz",
        OutputFormat::Brdb => "brdb",
    };
    PathBuf::from(&opts.output_directory).join(format!("{}.{}", opts.save_name, extension))
}
