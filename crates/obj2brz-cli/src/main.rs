use std::path::Path;
use std::process::ExitCode;

use lexopt::prelude::*;
use obj2brz::{
    convert, output_file_path, validate_obj_resources, BrickType, ConvertOptions, Logger, Material,
    OutputFormat,
};

const HELP: &str = "\
obj2brz - convert 3D models into Brickadia saves (BRZ/BRDB)

Supported inputs: Wavefront OBJ (.obj), STL (.stl), glTF (.gltf/.glb),
and FBX (.fbx).

USAGE:
    obj2brz [OPTIONS] <input-model>

OPTIONS:
    -o, --output-dir <DIR>       Output directory [default: .]
    -n, --name <NAME>            Save name [default: input file stem]
    -f, --format <FMT>           brz | brdb [default: brz]
        --bricktype <TYPE>       micro | default | tiles [default: micro]
        --material <MAT>         plastic | glass | glow | metallic | hologram | ghost [default: plastic]
        --material-intensity <N> 0-10 [default: 5]
        --no-player-collision     Do not block players
        --no-physics-collision    Do not collide with physics or brick grids
        --scale <F>              Overall scale multiplier [default: 1.0]
        --brick-scale <N>        Microbrick size multiplier [default: 1]
        --simplify               Lossy merge of similar bricks
        --squarish               Interior-seeded blocky packer (fewer seams)
        --posterize              Flatten textures to their essential colors
        --rampify                Generate default ramps directly from voxels
        --rampify-terrain        Rampify for terrain: only smooth top surfaces,
                                 undersides become plain bricks (implies --rampify)
        --no-corner-ramps        Rampify with straight ramps and wedges only
        --split-by-material      One frozen grid per OBJ material
        --grid-offset <X> <Y> <Z>  Spacing between material grids [default: 0 0 0]
        --owner-id <UUID>        Brick owner id
        --owner-name <NAME>      Brick owner name
        --skip-textures          Ignore textures, use flat material colors
    -q, --quiet                  Suppress progress output
    -h, --help                   Print this help
";

fn parse_bricktype(s: &str) -> Result<BrickType, String> {
    match s.to_ascii_lowercase().as_str() {
        "micro" | "microbricks" => Ok(BrickType::Microbricks),
        "default" => Ok(BrickType::Default),
        "tiles" | "tile" => Ok(BrickType::Tiles),
        other => Err(format!("unknown bricktype '{other}'")),
    }
}

fn parse_material(s: &str) -> Result<Material, String> {
    match s.to_ascii_lowercase().as_str() {
        "plastic" => Ok(Material::Plastic),
        "glass" => Ok(Material::Glass),
        "glow" => Ok(Material::Glow),
        "metallic" => Ok(Material::Metallic),
        "hologram" => Ok(Material::Hologram),
        "ghost" => Ok(Material::Ghost),
        other => Err(format!("unknown material '{other}'")),
    }
}

fn parse_format(s: &str) -> Result<OutputFormat, String> {
    match s.to_ascii_lowercase().as_str() {
        "brz" => Ok(OutputFormat::Brz),
        "brdb" => Ok(OutputFormat::Brdb),
        other => Err(format!("unknown format '{other}'")),
    }
}

fn run() -> Result<(), String> {
    let mut opts = ConvertOptions {
        output_directory: ".".into(),
        ..ConvertOptions::default()
    };
    let mut input: Option<String> = None;
    let mut name: Option<String> = None;
    let mut skip_textures = false;
    let mut quiet = false;

    let mut parser = lexopt::Parser::from_env();
    while let Some(arg) = parser.next().map_err(|e| e.to_string())? {
        match arg {
            Short('h') | Long("help") => {
                print!("{HELP}");
                return Ok(());
            }
            Short('o') | Long("output-dir") => {
                opts.output_directory = parser.value().map_err(|e| e.to_string())?.string().map_err(|e| e.to_string())?;
            }
            Short('n') | Long("name") => {
                name = Some(parser.value().map_err(|e| e.to_string())?.string().map_err(|e| e.to_string())?);
            }
            Short('f') | Long("format") => {
                opts.output_format = parse_format(&parser.value().map_err(|e| e.to_string())?.string().map_err(|e| e.to_string())?)?;
            }
            Long("bricktype") => {
                opts.bricktype = parse_bricktype(&parser.value().map_err(|e| e.to_string())?.string().map_err(|e| e.to_string())?)?;
            }
            Long("material") => {
                opts.material = parse_material(&parser.value().map_err(|e| e.to_string())?.string().map_err(|e| e.to_string())?)?;
            }
            Long("material-intensity") => {
                opts.material_intensity = parser.value().map_err(|e| e.to_string())?.parse().map_err(|e| e.to_string())?;
            }
            Long("no-player-collision") => opts.player_collision = false,
            Long("no-physics-collision") => opts.physics_collision = false,
            Long("scale") => {
                opts.scale = parser.value().map_err(|e| e.to_string())?.parse().map_err(|e| e.to_string())?;
            }
            Long("brick-scale") => {
                opts.brick_scale = parser.value().map_err(|e| e.to_string())?.parse().map_err(|e| e.to_string())?;
            }
            Long("simplify") => opts.simplify = true,
            Long("squarish") => opts.squarish = true,
            Long("posterize") => opts.posterize = true,
            Long("rampify") => opts.rampify = true,
            Long("rampify-terrain") => {
                opts.rampify = true;
                opts.rampify_terrain = true;
            }
            Long("no-corner-ramps") => opts.rampify_corners = false,
            Long("split-by-material") => opts.split_by_material = true,
            Long("grid-offset") => {
                opts.grid_offset_x = parser.value().map_err(|e| e.to_string())?.parse().map_err(|e| e.to_string())?;
                opts.grid_offset_y = parser.value().map_err(|e| e.to_string())?.parse().map_err(|e| e.to_string())?;
                opts.grid_offset_z = parser.value().map_err(|e| e.to_string())?.parse().map_err(|e| e.to_string())?;
            }
            Long("owner-id") => {
                opts.save_owner_id = parser.value().map_err(|e| e.to_string())?.string().map_err(|e| e.to_string())?;
            }
            Long("owner-name") => {
                opts.save_owner_name = parser.value().map_err(|e| e.to_string())?.string().map_err(|e| e.to_string())?;
            }
            Long("skip-textures") => skip_textures = true,
            Short('q') | Long("quiet") => quiet = true,
            Value(val) if input.is_none() => {
                input = Some(val.string().map_err(|e| e.to_string())?);
            }
            _ => return Err(format!("unexpected argument: {}", arg.unexpected())),
        }
    }

    let input = input.ok_or("missing required input model argument (see --help)")?;
    opts.input_file_path = input.clone();

    // Default save name to the input file stem.
    opts.save_name = name.unwrap_or_else(|| {
        Path::new(&input)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "out".to_string())
    });

    opts.logger = if quiet {
        Logger::new()
    } else {
        Logger::with_sink(|m| println!("{m}"))
    };

    if let Some(err) = opts.settings_error() {
        return Err(err);
    }

    let missing = validate_obj_resources(&opts.input_file_path).map_err(|e| e.to_string())?;
    if missing.has_issues() {
        eprintln!("Warning: missing resources:\n{}", missing.description());
        // Missing LDraw parts only skip that part's geometry; missing textures
        // would otherwise abort mid-conversion, so require an explicit opt-out.
        if !missing.missing_textures.is_empty() && !skip_textures {
            return Err("missing textures; re-run with --skip-textures to use flat material colors".into());
        }
    }

    convert(&opts, skip_textures).map_err(|e| e.to_string())?;
    println!("Wrote {}", output_file_path(&opts).display());
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
