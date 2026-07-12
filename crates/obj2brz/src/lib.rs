//! obj2brz core library: converts OBJ models into Brickadia saves (BRZ/BRDB).
//!
//! This crate is UI-agnostic. Build a [`ConvertOptions`] and call [`convert`];
//! progress is reported through the [`Logger`] carried on the options.

mod barycentric;
mod brdb_support;
mod convert;
mod error;
mod intersect;
mod logger;
mod octree;
mod rampify;
mod simplify;
mod voxelize;

pub use convert::{
    convert, model_bounds, output_file_path, BrickType, ConvertOptions, Material, ModelBounds,
    OutputFormat, SaveData,
};
pub use error::{ConversionError, ConversionResult, MissingResources};
pub use logger::Logger;

// Re-export brdb so front-ends can construct/inspect bricks without depending on
// the exact brdb path/version this crate pins.
pub use brdb;

pub use convert::validate_obj_resources;
