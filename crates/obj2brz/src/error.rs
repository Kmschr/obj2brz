use std::path::PathBuf;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum ConversionError {
    #[error("Failed to open OBJ file: {path}")]
    ObjFileNotFound { path: PathBuf },

    #[error("Failed to parse OBJ file: {0}")]
    ObjParseError(String),

    #[error("Failed to load texture {path}: {reason}")]
    TextureLoadError { path: PathBuf, reason: String },

    #[error("Failed to write save file: {0}")]
    SaveWriteError(String),

    #[error("Rampify could not allocate its voxel grid: {0}")]
    RampifyGridTooLarge(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Result type for conversion operations
pub type ConversionResult<T> = Result<T, ConversionError>;

/// Represents missing resources that the user should be notified about
#[derive(Debug)]
pub struct MissingResources {
    pub missing_textures: Vec<(String, PathBuf)>, // (material_name, texture_path)
    pub missing_materials: bool,
}

impl Default for MissingResources {
    fn default() -> Self {
        Self::new()
    }
}

impl MissingResources {
    pub fn new() -> Self {
        Self {
            missing_textures: Vec::new(),
            missing_materials: false,
        }
    }

    pub fn has_issues(&self) -> bool {
        !self.missing_textures.is_empty() || self.missing_materials
    }

    pub fn description(&self) -> String {
        let mut desc = String::new();

        if self.missing_materials {
            desc.push_str("• No material file (.mtl) found\n");
        }

        if !self.missing_textures.is_empty() {
            desc.push_str(&format!("• {} missing texture(s):\n\n", self.missing_textures.len()));
            for (material, path) in &self.missing_textures {
                desc.push_str(&format!("  Material: {}\n", material));
                desc.push_str(&format!("  Expected: {}\n\n",
                    path.display()));
            }
        }

        desc
    }
}
