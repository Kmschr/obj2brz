//! FBX (.fbx) model loader, built on the ufbx C library.
//!
//! ufbx normalizes the scene into right-handed Y-up space with one output
//! unit per meter, matching the pipeline's expectations. Triangles are
//! grouped by resolved material colour; each colour becomes one model with a
//! 1x1 solid-colour texture. Texture files are not sampled — the material's
//! base/diffuse colour is used instead.
//!
//! ufbx is C and cannot target wasm32-unknown-unknown, so the browser build
//! compiles stubs that report FBX as unsupported.

use std::path::Path;

pub fn is_fbx_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("fbx"))
}

/// Content sniff for byte-based entry points. Compiled on every target so
/// the browser build can recognize (and reject) FBX uploads.
pub fn looks_like_fbx(bytes: &[u8]) -> bool {
    if bytes.starts_with(b"Kaydara FBX Binary") {
        return true;
    }
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(2048)]);
    head.contains("FBXHeaderExtension")
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use crate::error::{ConversionError, ConversionResult};

    use std::collections::BTreeMap;
    use std::path::Path;

    pub fn load_fbx(path: &Path) -> ConversionResult<(Vec<tobj::Model>, Vec<image::RgbaImage>)> {
        let bytes = std::fs::read(path).map_err(|_| ConversionError::ObjFileNotFound {
            path: path.to_path_buf(),
        })?;
        load_fbx_bytes(&bytes)
    }

    pub fn load_fbx_bytes(bytes: &[u8]) -> ConversionResult<(Vec<tobj::Model>, Vec<image::RgbaImage>)> {
        let opts = ufbx::LoadOpts {
            target_axes: ufbx::CoordinateAxes::right_handed_y_up(),
            target_unit_meters: 1.0,
            generate_missing_normals: false,
            // Texture/material files referenced by the FBX are not needed:
            // only material colours are sampled.
            load_external_files: false,
            ignore_missing_external_files: true,
            ..Default::default()
        };
        let scene = ufbx::load_memory(bytes, opts)
            .map_err(|e| ConversionError::ObjParseError(format!("FBX: {}", e.description)))?;

        // Triangles grouped by resolved material colour.
        let mut triangles: BTreeMap<[u8; 4], Vec<f32>> = BTreeMap::new();

        for node in &scene.nodes {
            let Some(mesh) = &node.mesh else {
                continue;
            };
            let to_world = node.geometry_to_world;
            // Materials attach to the node when instanced, otherwise to the mesh.
            let materials = if node.materials.is_empty() {
                &mesh.materials
            } else {
                &node.materials
            };

            let mut scratch = vec![0u32; mesh.max_face_triangles.max(1) * 3];
            for (face_index, face) in mesh.faces.iter().enumerate() {
                let triangle_count = ufbx::triangulate_face(&mut scratch, mesh, *face);
                if triangle_count == 0 {
                    continue;
                }

                let color = mesh
                    .face_material
                    .get(face_index)
                    .and_then(|&material_index| materials.get(material_index as usize))
                    .map_or([255, 255, 255, 255], |material| material_color(material));

                let positions = triangles.entry(color).or_default();
                for &index in &scratch[..triangle_count as usize * 3] {
                    let local = mesh.vertex_position[index as usize];
                    let world = ufbx::transform_position(&to_world, local);
                    positions.push(world.x as f32);
                    positions.push(world.y as f32);
                    positions.push(world.z as f32);
                }
            }
        }

        if triangles.values().all(|positions| positions.is_empty()) {
            return Err(ConversionError::ObjParseError(
                "FBX contains no triangle geometry to voxelize".to_string(),
            ));
        }

        let mut models = Vec::new();
        let mut material_images = Vec::new();
        for (material_id, (rgba, positions)) in triangles.into_iter().enumerate() {
            let vertex_count = positions.len() / 3;
            let mesh = tobj::Mesh {
                positions,
                indices: (0..vertex_count as u32).collect(),
                material_id: Some(material_id),
                ..Default::default()
            };
            models.push(tobj::Model::new(mesh, format!("fbx_material_{material_id}")));
            material_images.push(image::RgbaImage::from_pixel(1, 1, image::Rgba(rgba)));
        }

        Ok((models, material_images))
    }

    /// Resolves a material to a flat colour: PBR base colour when present,
    /// then the classic FBX diffuse colour, then white.
    fn material_color(material: &ufbx::Material) -> [u8; 4] {
        let map = if material.pbr.base_color.has_value {
            &material.pbr.base_color
        } else if material.fbx.diffuse_color.has_value {
            &material.fbx.diffuse_color
        } else {
            return [255, 255, 255, 255];
        };
        let value = map.value_vec4;
        let alpha = if map.value_components >= 4 { value.w } else { 1.0 };
        [
            channel(value.x),
            channel(value.y),
            channel(value.z),
            channel(alpha),
        ]
    }

    fn channel(value: f64) -> u8 {
        (value.clamp(0.0, 1.0) * 255.0).round() as u8
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::{load_fbx, load_fbx_bytes};

#[cfg(target_arch = "wasm32")]
mod wasm_stub {
    use crate::error::{ConversionError, ConversionResult};

    use std::path::Path;

    const UNSUPPORTED: &str =
        "FBX is not supported in the browser; use the desktop app or CLI, or convert to glTF/OBJ";

    pub fn load_fbx(_path: &Path) -> ConversionResult<(Vec<tobj::Model>, Vec<image::RgbaImage>)> {
        Err(ConversionError::ObjParseError(UNSUPPORTED.to_string()))
    }

    pub fn load_fbx_bytes(
        _bytes: &[u8],
    ) -> ConversionResult<(Vec<tobj::Model>, Vec<image::RgbaImage>)> {
        Err(ConversionError::ObjParseError(UNSUPPORTED.to_string()))
    }
}

#[cfg(target_arch = "wasm32")]
pub use wasm_stub::{load_fbx, load_fbx_bytes};

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    #[test]
    fn sniffs_fbx() {
        assert!(looks_like_fbx(b"Kaydara FBX Binary  \x00rest"));
        assert!(looks_like_fbx(b"; FBX 7.3.0 project file\nFBXHeaderExtension:  {\n"));
        assert!(!looks_like_fbx(b"v 0 0 0\nf 1 2 3\n"));
        assert!(is_fbx_path("model.FBX"));
        assert!(!is_fbx_path("model.obj"));
    }
}
