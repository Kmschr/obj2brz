//! STL (.stl) model loader, supporting both binary and ASCII variants.
//!
//! STL carries bare triangles with no colors, UVs, or materials, so the
//! loader produces a single model that the pipeline paints with the default
//! white material. STL models are conventionally Z-up (CAD/3D printing);
//! vertices are rotated into the Y-up space the voxelizer expects.

use crate::error::{ConversionError, ConversionResult};

use std::path::Path;

pub fn is_stl_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("stl"))
}

/// Content sniff for byte-based entry points. Binary STL is recognized by its
/// exact size formula, ASCII by its `solid`/`facet` keywords.
pub fn looks_like_stl(bytes: &[u8]) -> bool {
    if binary_triangle_count(bytes).is_some() {
        return true;
    }
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(2048)]);
    let head = head.trim_start();
    head.starts_with("solid") && head.contains("facet")
}

pub fn load_stl(path: &Path) -> ConversionResult<Vec<tobj::Model>> {
    let bytes = std::fs::read(path).map_err(|_| ConversionError::ObjFileNotFound {
        path: path.to_path_buf(),
    })?;
    load_stl_bytes(&bytes)
}

pub fn load_stl_bytes(bytes: &[u8]) -> ConversionResult<Vec<tobj::Model>> {
    // Check the binary size formula first: binary files routinely start with
    // the bytes "solid" in their free-form 80-byte header.
    let positions = if let Some(triangle_count) = binary_triangle_count(bytes) {
        parse_binary(bytes, triangle_count)
    } else {
        parse_ascii(bytes)?
    };

    if positions.is_empty() {
        return Err(ConversionError::ObjParseError(
            "STL contains no triangle geometry to voxelize".to_string(),
        ));
    }

    let vertex_count = positions.len() / 3;
    let mesh = tobj::Mesh {
        positions,
        indices: (0..vertex_count as u32).collect(),
        ..Default::default()
    };
    Ok(vec![tobj::Model::new(mesh, "stl".to_string())])
}

/// Returns the triangle count if `bytes` matches the binary STL layout:
/// an 80-byte header, a u32 triangle count, then 50 bytes per triangle.
fn binary_triangle_count(bytes: &[u8]) -> Option<u32> {
    let count_bytes = bytes.get(80..84)?;
    let count = u32::from_le_bytes(count_bytes.try_into().unwrap());
    (84 + count as usize * 50 == bytes.len()).then_some(count)
}

fn parse_binary(bytes: &[u8], triangle_count: u32) -> Vec<f32> {
    let mut positions = Vec::with_capacity(triangle_count as usize * 9);
    for triangle in bytes[84..].chunks_exact(50) {
        // Skip the 12-byte facet normal; each vertex is three f32s.
        for vertex in triangle[12..48].chunks_exact(12) {
            push_z_up_vertex(&mut positions, [
                f32::from_le_bytes(vertex[0..4].try_into().unwrap()),
                f32::from_le_bytes(vertex[4..8].try_into().unwrap()),
                f32::from_le_bytes(vertex[8..12].try_into().unwrap()),
            ]);
        }
    }
    positions
}

fn parse_ascii(bytes: &[u8]) -> ConversionResult<Vec<f32>> {
    let text = String::from_utf8_lossy(bytes);
    let mut positions = Vec::new();
    let mut tokens = text.split_whitespace();
    while let Some(token) = tokens.next() {
        if token != "vertex" {
            continue;
        }
        let mut vertex = [0.0f32; 3];
        for value in &mut vertex {
            *value = tokens
                .next()
                .and_then(|t| t.parse().ok())
                .ok_or_else(|| {
                    ConversionError::ObjParseError("malformed vertex in ASCII STL".to_string())
                })?;
        }
        push_z_up_vertex(&mut positions, vertex);
    }
    Ok(positions)
}

/// STL is conventionally Z-up; the pipeline expects Y-up. Rotate -90 degrees
/// about X: (x, y, z) -> (x, z, -y).
fn push_z_up_vertex(positions: &mut Vec<f32>, [x, y, z]: [f32; 3]) {
    positions.push(x);
    positions.push(z);
    positions.push(-y);
}

#[cfg(test)]
mod tests {
    use super::*;

    const ASCII_TRIANGLE: &str = "\
solid test
  facet normal 0 0 1
    outer loop
      vertex 0 0 0
      vertex 2 0 0
      vertex 0 3 1
    endloop
  endfacet
endsolid test
";

    fn binary_stl(triangles: &[[f32; 9]]) -> Vec<u8> {
        let mut bytes = vec![0u8; 80];
        bytes.extend_from_slice(&(triangles.len() as u32).to_le_bytes());
        for triangle in triangles {
            bytes.extend_from_slice(&[0u8; 12]);
            for value in triangle {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
            bytes.extend_from_slice(&[0u8; 2]);
        }
        bytes
    }

    #[test]
    fn parses_ascii_and_converts_to_y_up() {
        let models = load_stl_bytes(ASCII_TRIANGLE.as_bytes()).unwrap();
        assert_eq!(models.len(), 1);
        let positions = &models[0].mesh.positions;
        // (0,3,1) z-up becomes (0,1,-3) y-up.
        assert_eq!(&positions[6..9], &[0.0, 1.0, -3.0]);
    }

    #[test]
    fn parses_binary() {
        let bytes = binary_stl(&[[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]]);
        let models = load_stl_bytes(&bytes).unwrap();
        assert_eq!(models[0].mesh.positions.len(), 9);
        assert_eq!(models[0].mesh.indices, vec![0, 1, 2]);
    }

    #[test]
    fn sniffs_both_variants() {
        assert!(looks_like_stl(ASCII_TRIANGLE.as_bytes()));
        assert!(looks_like_stl(&binary_stl(&[[0.0; 9]])));
        assert!(!looks_like_stl(b"v 0 0 0\nf 1 2 3\n"));
        // Binary header that merely starts with "solid" still parses as binary.
        let mut deceptive = binary_stl(&[[0.0; 9]]);
        deceptive[..5].copy_from_slice(b"solid");
        assert!(binary_triangle_count(&deceptive).is_some());
    }

    #[test]
    fn empty_stl_is_an_error() {
        assert!(load_stl_bytes(b"solid empty\nendsolid empty\n").is_err());
    }
}
