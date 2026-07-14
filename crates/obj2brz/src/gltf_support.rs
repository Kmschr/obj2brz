//! glTF (.gltf/.glb) model loader.
//!
//! glTF is already right-handed Y-up in meters, so geometry passes through
//! with only the node hierarchy's world transforms applied. Materials map to
//! the pipeline's texture list: primitives with an embedded base-colour
//! texture keep it (and their UVs), everything else gets a 1x1 texture of
//! the material's base-colour factor.

use crate::error::{ConversionError, ConversionResult};

use std::path::Path;

pub fn is_gltf_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("gltf") || extension.eq_ignore_ascii_case("glb")
        })
}

/// Content sniff for byte-based entry points: GLB has a magic header, and a
/// .gltf upload is a JSON document (which no other supported format is).
pub fn looks_like_gltf(bytes: &[u8]) -> bool {
    if bytes.starts_with(b"glTF") {
        return true;
    }
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(2048)]);
    let head = head.trim_start();
    head.starts_with('{') && head.contains("\"asset\"")
}

pub fn load_gltf(path: &Path) -> ConversionResult<(Vec<tobj::Model>, Vec<image::RgbaImage>)> {
    let (document, buffers, images) = gltf::import(path)
        .map_err(|e| ConversionError::ObjParseError(format!("glTF: {e}")))?;
    build_models(&document, &buffers, &images)
}

pub fn load_gltf_bytes(bytes: &[u8]) -> ConversionResult<(Vec<tobj::Model>, Vec<image::RgbaImage>)> {
    let (document, buffers, images) = gltf::import_slice(bytes)
        .map_err(|e| ConversionError::ObjParseError(format!("glTF: {e}")))?;
    build_models(&document, &buffers, &images)
}

fn build_models(
    document: &gltf::Document,
    buffers: &[gltf::buffer::Data],
    images: &[gltf::image::Data],
) -> ConversionResult<(Vec<tobj::Model>, Vec<image::RgbaImage>)> {
    // One texture slot per document material, plus a trailing white slot for
    // primitives that use the default material.
    let mut material_images: Vec<image::RgbaImage> = document
        .materials()
        .map(|material| material_image(&material, images))
        .collect();
    material_images.push(image::RgbaImage::from_pixel(
        1,
        1,
        image::Rgba([255, 255, 255, 255]),
    ));
    let default_material_id = material_images.len() - 1;

    let mut models = Vec::new();
    let scene = document
        .default_scene()
        .or_else(|| document.scenes().next());
    let Some(scene) = scene else {
        return Err(ConversionError::ObjParseError(
            "glTF contains no scene".to_string(),
        ));
    };

    for node in scene.nodes() {
        collect_node(&node, IDENTITY, buffers, default_material_id, &mut models);
    }

    if !models
        .iter()
        .any(|model| model.mesh.indices.len() >= 3 && model.mesh.positions.len() >= 3)
    {
        return Err(ConversionError::ObjParseError(
            "glTF contains no triangle geometry to voxelize".to_string(),
        ));
    }

    Ok((models, material_images))
}

const IDENTITY: [[f32; 4]; 4] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

fn collect_node(
    node: &gltf::Node,
    parent: [[f32; 4]; 4],
    buffers: &[gltf::buffer::Data],
    default_material_id: usize,
    models: &mut Vec<tobj::Model>,
) {
    let world = multiply(parent, node.transform().matrix());

    if let Some(mesh) = node.mesh() {
        for primitive in mesh.primitives() {
            if primitive.mode() != gltf::mesh::Mode::Triangles {
                continue;
            }
            let reader = primitive.reader(|buffer| buffers.get(buffer.index()).map(|d| &*d.0));
            let Some(positions) = reader.read_positions() else {
                continue;
            };

            let mut flat_positions = Vec::new();
            for position in positions {
                let world_position = transform_point(&world, position);
                flat_positions.extend_from_slice(&world_position);
            }
            let vertex_count = (flat_positions.len() / 3) as u32;

            let indices: Vec<u32> = match reader.read_indices() {
                Some(indices) => indices.into_u32().collect(),
                None => (0..vertex_count).collect(),
            };

            // glTF UVs have a top-left origin; the voxelizer flips V when it
            // samples (OBJ convention), so pre-flip to cancel that out.
            let texcoords: Vec<f32> = reader
                .read_tex_coords(0)
                .map(|coords| {
                    coords
                        .into_f32()
                        .flat_map(|[u, v]| [u, 1.0 - v])
                        .collect()
                })
                .unwrap_or_default();

            let material_id = primitive.material().index().unwrap_or(default_material_id);
            let mesh = tobj::Mesh {
                positions: flat_positions,
                indices,
                texcoords,
                material_id: Some(material_id),
                ..Default::default()
            };
            models.push(tobj::Model::new(mesh, node.name().unwrap_or("gltf").to_string()));
        }
    }

    for child in node.children() {
        collect_node(&child, world, buffers, default_material_id, models);
    }
}

/// Resolves a material to a pipeline texture: its base-colour texture when
/// the image decoded, otherwise a 1x1 of its base-colour factor.
fn material_image(material: &gltf::Material, images: &[gltf::image::Data]) -> image::RgbaImage {
    let pbr = material.pbr_metallic_roughness();

    if let Some(info) = pbr.base_color_texture() {
        let image_index = info.texture().source().index();
        if let Some(decoded) = images.get(image_index).and_then(to_rgba_image) {
            return decoded;
        }
    }

    let factor = pbr.base_color_factor();
    image::RgbaImage::from_pixel(
        1,
        1,
        image::Rgba([
            channel(factor[0]),
            channel(factor[1]),
            channel(factor[2]),
            channel(factor[3]),
        ]),
    )
}

/// Converts gltf's decoded pixel data into the image-0.24 RgbaImage the
/// voxelizer samples. Uncommon channel layouts fall back to the factor colour.
fn to_rgba_image(data: &gltf::image::Data) -> Option<image::RgbaImage> {
    use gltf::image::Format;

    let pixel_count = (data.width * data.height) as usize;
    let rgba = match data.format {
        Format::R8G8B8A8 => data.pixels.clone(),
        Format::R8G8B8 => {
            let mut rgba = Vec::with_capacity(pixel_count * 4);
            for rgb in data.pixels.chunks_exact(3) {
                rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
            }
            rgba
        }
        _ => return None,
    };
    image::RgbaImage::from_raw(data.width, data.height, rgba)
}

fn channel(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn multiply(a: [[f32; 4]; 4], b: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
    // glTF matrices are column-major: m[column][row].
    let mut out = [[0.0; 4]; 4];
    for (column, out_column) in out.iter_mut().enumerate() {
        for row in 0..4 {
            out_column[row] = (0..4).map(|k| a[k][row] * b[column][k]).sum();
        }
    }
    out
}

fn transform_point(m: &[[f32; 4]; 4], [x, y, z]: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * x + m[1][0] * y + m[2][0] * z + m[3][0],
        m[0][1] * x + m[1][1] * y + m[2][1] * z + m[3][1],
        m[0][2] * x + m[1][2] * y + m[2][2] * z + m[3][2],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_gltf() {
        assert!(looks_like_gltf(b"glTF\x02\x00\x00\x00rest"));
        assert!(looks_like_gltf(b"{ \"asset\": { \"version\": \"2.0\" } }"));
        assert!(!looks_like_gltf(b"v 0 0 0\nf 1 2 3\n"));
        assert!(is_gltf_path("model.glb"));
        assert!(is_gltf_path("model.GLTF"));
        assert!(!is_gltf_path("model.obj"));
    }

    #[test]
    fn loads_embedded_gltf_json() {
        // Single triangle, base64-embedded buffer: three vec3 positions.
        let gltf_json = r#"{
            "asset": {"version": "2.0"},
            "scene": 0,
            "scenes": [{"nodes": [0]}],
            "nodes": [{"mesh": 0, "translation": [1.0, 0.0, 0.0]}],
            "meshes": [{"primitives": [{"attributes": {"POSITION": 0}}]}],
            "accessors": [{
                "bufferView": 0, "componentType": 5126, "count": 3,
                "type": "VEC3", "min": [0,0,0], "max": [1,1,0]
            }],
            "bufferViews": [{"buffer": 0, "byteLength": 36}],
            "buffers": [{
                "byteLength": 36,
                "uri": "data:application/octet-stream;base64,AAAAAAAAAAAAAAAAAACAPwAAAAAAAAAAAAAAAAAAgD8AAAAA"
            }]
        }"#;
        let (models, images) = load_gltf_bytes(gltf_json.as_bytes()).unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].mesh.positions.len(), 9);
        // Node translation applied: x shifted by 1.
        assert_eq!(models[0].mesh.positions[0], 1.0);
        // No document materials: single default white slot.
        assert_eq!(images.len(), 1);
        assert_eq!(models[0].mesh.material_id, Some(0));
    }
}
