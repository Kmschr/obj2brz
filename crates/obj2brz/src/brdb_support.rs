use crate::grid_mesh::GridMeshStats;
use crate::{ConvertOptions, OutputFormat, SaveData};
use crate::error::{ConversionError, ConversionResult};
use std::path::{Path, PathBuf};
use brdb::{assets, Brick, Brz, BundleAuthor, Entity, Guid, IntoReader, Owner, World};
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
    if let Some(first) = full_path.get_mut(0..1) {
        first.make_ascii_lowercase();
    }

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

/// Builds a fully configured single-grid [`World`] from save data. Shared by the
/// native (write-to-disk) and browser (encode-to-bytes) paths.
fn world_from_save_data(
    name: &str,
    data: &SaveData,
    opts: &ConvertOptions,
    preview_image: Option<Vec<u8>>,
) -> ConversionResult<World> {
    let mut world = World::new();

    if let Some(img) = preview_image {
        world.meta.screenshot = Some(img);
    }

    world.meta.bundle.name = name.to_string();
    world.meta.bundle.authors = vec![BundleAuthor {
        id: opts.save_owner_id.clone(),
        name: data.author_name.clone(),
    }];
    world.meta.bundle.description = "Converted with obj2brz".to_string();

    // Copy bricks directly - they're already in brdb format.
    world.bricks = data.bricks.clone();
    configure_world(&mut world, opts)?;
    Ok(world)
}

/// Encodes a configured world into BRZ bytes without touching the filesystem.
fn world_to_brz_bytes(world: &World) -> ConversionResult<Vec<u8>> {
    fn inner(world: &World) -> Result<Vec<u8>, brdb::BrError> {
        // Level 6 mirrors the native path: near level-14 size, far faster.
        Ok(world
            .to_unsaved()?
            .to_pending()?
            .to_brz_data(Some(6))?
            .to_vec(Some(6))?)
    }

    inner(world).map_err(|e| ConversionError::SaveWriteError(format!("Failed to encode BRZ: {e}")))
}

/// Encodes single-grid save data straight to BRZ bytes (browser build).
pub fn brz_bytes(
    name: &str,
    data: &SaveData,
    opts: &ConvertOptions,
    preview_image: Option<Vec<u8>>,
) -> ConversionResult<Vec<u8>> {
    let world = world_from_save_data(name, data, opts, preview_image)?;
    world_to_brz_bytes(&world)
}

pub fn write_brz(
    path: PathBuf,
    data: &SaveData,
    opts: &ConvertOptions,
    _use_procedural: bool,
    preview_image: Option<Vec<u8>>,
) -> ConversionResult<()> {
    let name = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_default();
    let world = world_from_save_data(&name, data, opts, preview_image)?;
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

fn grid_mesh_world(
    name: &str,
    anchor: Brick,
    grids: Vec<(Entity, Vec<Brick>)>,
    stats: &GridMeshStats,
    opts: &ConvertOptions,
    preview_image: Option<Vec<u8>>,
) -> ConversionResult<World> {
    let mut world = World::new();
    world.meta.screenshot = preview_image;
    world.meta.bundle.name = name.to_string();
    world.meta.bundle.authors = vec![BundleAuthor {
        id: opts.save_owner_id.clone(),
        name: opts.save_owner_name.clone(),
    }];
    world.meta.bundle.description = format!(
        "Converted with obj2brz experimental grid meshes: {} source faces, {} subdivided, \
         {} micro-wedges grouped onto {} frozen coplanar grids; endpoint RMS {:.3}, maximum {:.3} Brickadia units. \
         The sole main-grid brick is an invisible, non-colliding prefab-bounds anchor.",
        stats.source_triangles,
        stats.subdivided_source_triangles,
        stats.emitted_wedges,
        stats.emitted_grids,
        stats.rms_endpoint_error(),
        stats.max_endpoint_error,
    );
    world.bricks.push(anchor);
    for (entity, bricks) in grids {
        world.add_brick_grid(entity, bricks);
    }

    configure_world(&mut world, opts)?;
    // Grid entities require their full entity/component schema. This also
    // follows the triangle-plane experiment's known-good writer sequence.
    world.register_all_components();
    world.make_prefab();
    Ok(world)
}

pub fn write_grid_mesh(
    path: PathBuf,
    anchor: Brick,
    grids: Vec<(Entity, Vec<Brick>)>,
    stats: &GridMeshStats,
    opts: &ConvertOptions,
    preview_image: Option<Vec<u8>>,
) -> ConversionResult<()> {
    let name = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_default();
    let expected_grids = stats.emitted_grids;
    let expected_wedges = stats.emitted_wedges;
    let world = grid_mesh_world(&name, anchor, grids, stats, opts, preview_image)?;
    write_world(path.clone(), world, opts)?;
    if opts.output_format == OutputFormat::Brz {
        validate_grid_mesh_brz(&path, expected_grids, expected_wedges)?;
        opts.logger.log(format!(
            "Validated prefab anchor, {} wedges, and {} frozen grid transforms",
            expected_wedges, expected_grids
        ));
    }
    Ok(())
}

pub fn grid_mesh_brz_bytes(
    name: &str,
    anchor: Brick,
    grids: Vec<(Entity, Vec<Brick>)>,
    stats: &GridMeshStats,
    opts: &ConvertOptions,
    preview_image: Option<Vec<u8>>,
) -> ConversionResult<Vec<u8>> {
    let world = grid_mesh_world(name, anchor, grids, stats, opts, preview_image)?;
    world_to_brz_bytes(&world)
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
        // Metadata-only bounds anchors are deliberately invisible and must
        // remain non-colliding regardless of the visible-brick preferences.
        if brick.visible {
            brick.collision.player = opts.player_collision;
            brick.collision.physics = opts.physics_collision;
        }
    }
    for (entity, bricks) in &mut world.grids {
        entity.owner_index = Some(1);
        entity.original_owner_index = Some(1);
        for brick in bricks {
            brick.owner_index = Some(1);
            brick.original_owner_index = Some(1);
            brick.material_intensity = opts.material_intensity as u8;
            brick.collision.player = opts.player_collision;
            brick.collision.physics = opts.physics_collision;
        }
    }

    // Newer BRDB versions require generated prefabs to carry Prefab.json, and
    // frozen grids need their entity schema registered before serialization.
    world.make_prefab();
    world.register_used_components();
    Ok(())
}

fn validate_grid_mesh_brz(
    path: &Path,
    expected_grids: usize,
    expected_wedges: usize,
) -> ConversionResult<()> {
    fn inner(
        path: &Path,
        expected_grids: usize,
        expected_wedges: usize,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let reader = Brz::open(path)?.into_reader();
        if reader.prefab_json()?.is_none() {
            return Err("grid mesh is missing Meta/Prefab.json".into());
        }
        let global = reader.global_data()?;

        let mut main_count = 0;
        for chunk in reader.brick_chunk_index(1)? {
            let bricks = reader.brick_chunk_soa(1, chunk.index)?;
            main_count += bricks.brick_type_indices.len();
            for counter in bricks.brick_size_counters {
                let asset = global
                    .procedural_brick_asset_names
                    .get_index(counter.asset_index as usize);
                if asset.map(String::as_str)
                    != Some(assets::bricks::PB_DEFAULT_MICRO_BRICK.as_ref())
                {
                    return Err("grid mesh main-grid anchor is not a microbrick".into());
                }
            }
        }
        if main_count != 1 {
            return Err(format!(
                "grid mesh has {main_count} main-grid bricks, expected one anchor"
            )
            .into());
        }

        let mut wedge_count = 0;
        for grid_id in 2..=(expected_grids + 1) {
            let mut brick_count = 0;
            for chunk in reader.brick_chunk_index(grid_id)? {
                let bricks = reader.brick_chunk_soa(grid_id, chunk.index)?;
                brick_count += bricks.brick_type_indices.len();
                for counter in bricks.brick_size_counters {
                    let asset = global
                        .procedural_brick_asset_names
                        .get_index(counter.asset_index as usize);
                    if asset.map(String::as_str)
                        != Some(assets::bricks::PB_DEFAULT_MICRO_WEDGE.as_ref())
                    {
                        return Err(format!(
                            "grid mesh dynamic grid {grid_id} does not contain a micro-wedge"
                        )
                        .into());
                    }
                }
            }
            if brick_count == 0 {
                return Err(format!(
                    "grid mesh dynamic grid {grid_id} has no micro-wedges"
                )
                .into());
            }
            wedge_count += brick_count;
        }
        if wedge_count != expected_wedges {
            return Err(format!(
                "grid mesh has {wedge_count} micro-wedges, expected {expected_wedges}"
            )
            .into());
        }

        let mut entity_count = 0;
        for chunk in reader.entity_chunk_index()? {
            for entity in reader.entity_chunk(chunk)? {
                entity_count += 1;
                let q = entity.rotation;
                let magnitude = (q.x * q.x + q.y * q.y + q.z * q.z + q.w * q.w).sqrt();
                if !entity.frozen
                    || !entity.sleeping
                    || !magnitude.is_finite()
                    || (magnitude - 1.0).abs() > 0.001
                {
                    return Err(format!(
                        "grid mesh entity {entity_count} has an invalid frozen transform"
                    )
                    .into());
                }
            }
        }
        if entity_count != expected_grids {
            return Err(format!(
                "grid mesh has {entity_count} entities, expected {expected_grids}"
            )
            .into());
        }
        Ok(())
    }

    inner(path, expected_grids, expected_wedges).map_err(|error| {
        ConversionError::SaveWriteError(format!(
            "grid mesh validation failed for {}: {error}",
            path.display()
        ))
    })
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

    #[test]
    fn applies_requested_collision_settings_to_every_brick() {
        let mut world = World::new();
        world.bricks.push(Brick::default());
        let opts = ConvertOptions {
            player_collision: false,
            physics_collision: false,
            ..ConvertOptions::default()
        };

        configure_world(&mut world, &opts).unwrap();

        assert!(!world.bricks[0].collision.player);
        assert!(!world.bricks[0].collision.physics);
    }
}
