use crate::{ConvertOptions, OutputFormat, SaveData};
use crate::error::{ConversionError, ConversionResult};
use std::path::{Path, PathBuf};
use brdb::{Brz, BundleAuthor, Entity, Guid, Owner, World};
use uuid::Uuid;

fn copy_path_to_clipboard(path: &Path, opts: &ConvertOptions) -> ConversionResult<()> {
    if !opts.copy_to_clipboard {
        return Ok(());
    }

    // Get absolute path
    let mut full_path = path
        .canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .to_string();

    // Lowercase the first letter (drive letter on Windows)
    full_path.get_mut(0..1).map(|s| s.make_ascii_lowercase());

    #[cfg(target_os = "windows")]
    {
        clipboard_win::raw::open()
            .map_err(|e| ConversionError::SaveWriteError(format!("Failed to open clipboard: {}", e)))?;

        clipboard_win::raw::set_file_list(&[full_path.clone()])
            .map_err(|e| {
                let _ = clipboard_win::raw::close();
                ConversionError::SaveWriteError(format!("Failed to set clipboard: {}", e))
            })?;

        clipboard_win::raw::close()
            .map_err(|e| ConversionError::SaveWriteError(format!("Failed to close clipboard: {}", e)))?;

        opts.logger.log(format!("Copied path to clipboard: {}", full_path));
    }

    #[cfg(not(target_os = "windows"))]
    {
        opts.logger.log("Clipboard file path support is only available on Windows".to_string());
        opts.logger.log(format!("File saved to: {}", full_path));
    }

    Ok(())
}

pub fn write_brz(
    path: PathBuf,
    data: &SaveData,
    opts: &ConvertOptions,
    _use_procedural: bool,
    preview_image: Option<Vec<u8>>,
) -> ConversionResult<()> {
    let mut world = World::new();

    // Set Metadata
    if let Some(img) = preview_image {
        world.meta.screenshot = Some(img);
    }

    // Set Bundle Info from SaveData
    if let Some(stem) = path.file_stem() {
        world.meta.bundle.name = stem.to_string_lossy().to_string();
    }
    world.meta.bundle.authors = vec![BundleAuthor {
        id: opts.save_owner_id.clone(),
        name: data.author_name.clone(),
    }];
    world.meta.bundle.description = "Converted with obj2brz".to_string();

    // Copy bricks directly - they're already in brdb format.
    world.bricks = data.bricks.clone();
    configure_world(&mut world, opts)?;
    write_world(path, world, opts)
}

pub fn write_brz_grids(
    path: PathBuf,
    grids: Vec<(Entity, Vec<brdb::Brick>)>,
    opts: &ConvertOptions,
    preview_image: Option<Vec<u8>>,
) -> ConversionResult<()> {
    let mut world = World::new();

    // Set Metadata
    if let Some(img) = preview_image {
        world.meta.screenshot = Some(img);
    }

    // Set Bundle Info
    if let Some(stem) = path.file_stem() {
        world.meta.bundle.name = stem.to_string_lossy().to_string();
    }
    world.meta.bundle.authors = vec![BundleAuthor {
        id: opts.save_owner_id.clone(),
        name: opts.save_owner_name.clone(),
    }];
    world.meta.bundle.description = "Converted with obj2brz (split by material)".to_string();

    // Add each material's bricks as a separate frozen grid
    let total_bricks: usize = grids.iter().map(|(_, bricks)| bricks.len()).sum();
    for (entity, bricks) in grids {
        world.add_brick_grid(entity, bricks);
    }

    configure_world(&mut world, opts)?;

    opts.logger.log(format!("Total bricks across all grids: {}", total_bricks));

    write_world(path, world, opts)
}

fn configure_world(world: &mut World, opts: &ConvertOptions) -> ConversionResult<()> {
    let owner_id = Uuid::parse_str(&opts.save_owner_id)
        .map(Guid::from)
        .map_err(|e| ConversionError::SaveWriteError(format!("Invalid owner UUID: {e}")))?;

    // Owner index 0 is Brickadia's public owner. Generated content belongs to
    // the requested owner at index 1, including frozen-grid entities.
    world.owners.insert(owner_id, Owner {
        user_id: owner_id,
        user_name: opts.save_owner_name.clone(),
        display_name: opts.save_owner_name.clone(),
    });
    for brick in &mut world.bricks {
        brick.owner_index = Some(1);
        brick.original_owner_index = Some(1);
        brick.material_intensity = opts.material_intensity as u8;
    }
    for (entity, bricks) in &mut world.grids {
        entity.owner_index = Some(1);
        entity.original_owner_index = Some(1);
        for brick in bricks {
            brick.owner_index = Some(1);
            brick.original_owner_index = Some(1);
            brick.material_intensity = opts.material_intensity as u8;
        }
    }

    // Newer BRDB versions require generated prefabs to carry Prefab.json, and
    // frozen grids need their entity schema registered before serialization.
    world.make_prefab();
    world.register_used_components();
    Ok(())
}

fn write_world(path: PathBuf, world: World, opts: &ConvertOptions) -> ConversionResult<()> {
    let result = match opts.output_format {
        OutputFormat::Brz => {
            // Level 6 is near the size of level 14, but dramatically faster on
            // large voxel imports (the default used by World::write_brz).
            Brz::save_with_level(&path, &world, Some(6))
        }
        OutputFormat::Brdb => world.write_brdb(&path),
    };

    result.map_err(|e| ConversionError::SaveWriteError(format!("Failed to write save file: {e}")))?;
    opts.logger.log(format!("Successfully wrote save to {:?}", path));
    copy_path_to_clipboard(&path, opts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use brdb::Brick;

    #[test]
    fn writes_a_prefab_brz_with_the_current_brdb_writer() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "obj2brz-{}-{}.brz",
            std::process::id(),
            nonce
        ));
        let opts = ConvertOptions::default();
        let data = SaveData {
            bricks: vec![Brick::default()],
            author_name: opts.save_owner_name.clone(),
        };

        write_brz(path.clone(), &data, &opts, false, None).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.starts_with(b"BRZ"));
        std::fs::remove_file(path).unwrap();
    }
}
