//! LDraw (.dat/.ldr/.mpd) model loader.
//!
//! Parses LDraw geometry (triangles, quads, and recursive subfile references)
//! into the same `tobj::Model` + material-image structures the OBJ loader
//! produces, so the rest of the pipeline stays format-agnostic. Triangles are
//! grouped by resolved LDraw colour; each colour becomes one model with a
//! 1x1 solid-colour texture, mirroring how untextured OBJ materials work.
//!
//! LDraw space is right-handed with -Y up and measured in LDU (20 LDU = one
//! stud). Vertices are rotated 180 degrees about X (x, -y, -z) into the Y-up
//! space the voxelizer expects and scaled from LDU to studs.

use crate::error::{ConversionError, ConversionResult};
use crate::logger::Logger;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

const LDU_PER_STUD: f32 = 20.0;
const MAX_RECURSION_DEPTH: usize = 64;
/// Colour code 16: inherit the referencing file's colour.
const CODE_INHERIT: u32 = 16;
/// Colour code 24: the complement (edge) colour. Edges are never rendered
/// here, so any stray use behaves like inherit.
const CODE_EDGE_INHERIT: u32 = 24;
/// Colour used for the top level and for codes we cannot resolve.
const FALLBACK_COLOR: [u8; 4] = [0x9B, 0xA1, 0x9D, 255];

pub fn is_ldraw_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "dat" | "ldr" | "mpd"
            )
        })
}

/// Content sniff for byte-based entry points (the browser build), which have
/// no file extension to dispatch on. Every LDraw line begins with a 0-5 line
/// type, which no meaningful OBJ line does.
pub fn looks_like_ldraw(bytes: &[u8]) -> bool {
    let sample = &bytes[..bytes.len().min(64 * 1024)];
    let text = String::from_utf8_lossy(sample);
    let mut ldraw_lines = 0usize;
    let mut other_lines = 0usize;
    for line in text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(200)
    {
        match line.split_whitespace().next() {
            Some("0" | "1" | "2" | "3" | "4" | "5") => ldraw_lines += 1,
            _ => other_lines += 1,
        }
    }
    ldraw_lines > other_lines
}

/// Result of loading an LDraw file, shaped to slot into the OBJ pipeline.
pub struct LDrawModel {
    pub models: Vec<tobj::Model>,
    pub material_images: Vec<image::RgbaImage>,
    /// Subfile references that could not be resolved (missing parts).
    pub missing_subfiles: Vec<String>,
}

/// Loads an LDraw file from disk, resolving subfile references against the
/// file's own directory and any installed LDraw parts library.
pub fn load_ldraw(path: &Path, logger: &Logger) -> ConversionResult<LDrawModel> {
    let text = std::fs::read_to_string(path).map_err(|_| ConversionError::ObjFileNotFound {
        path: path.to_path_buf(),
    })?;

    let mut search_dirs = Vec::new();
    if let Some(parent) = path.parent() {
        search_dirs.push(parent.to_path_buf());
    }

    let mut library_roots = library_roots();
    // A file opened from inside a library checkout (e.g. <lib>/parts/x.dat)
    // must resolve primitives against that library, wherever it lives.
    for ancestor in path.ancestors().skip(1) {
        if is_library_root(ancestor) {
            library_roots.insert(0, ancestor.to_path_buf());
            break;
        }
    }
    for root in &library_roots {
        for sub in ["parts", "p", "models"] {
            search_dirs.push(root.join(sub));
        }
        search_dirs.push(root.clone());
    }

    let mut loader = Loader::new(search_dirs, logger.clone());
    for root in &library_roots {
        loader.load_ldconfig(&root.join("LDConfig.ldr"));
    }
    loader.load(&text)
}

/// Loads an LDraw model straight from bytes for the browser build. Only
/// subfiles embedded in the data itself (MPD documents) can be resolved.
pub fn load_ldraw_bytes(bytes: &[u8], logger: &Logger) -> ConversionResult<LDrawModel> {
    let text = String::from_utf8_lossy(bytes).into_owned();
    let loader = Loader::new(Vec::new(), logger.clone());
    loader.load(&text)
}

/// One parsed LDraw command. Line types 2 (line) and 5 (optional line) carry
/// no surface geometry and are dropped at parse time.
enum Command {
    SubFile {
        color: u32,
        transform: Transform,
        name: String,
    },
    Triangle {
        color: u32,
        vertices: [[f32; 3]; 3],
    },
    Quad {
        color: u32,
        vertices: [[f32; 3]; 4],
    },
    ColourDef {
        code: u32,
        rgba: [u8; 4],
    },
}

/// Row-major affine transform: `p' = m * p + t`.
#[derive(Clone, Copy)]
struct Transform {
    m: [[f32; 3]; 3],
    t: [f32; 3],
}

impl Transform {
    const IDENTITY: Self = Self {
        m: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        t: [0.0, 0.0, 0.0],
    };

    fn apply(&self, p: [f32; 3]) -> [f32; 3] {
        let mut out = self.t;
        for (row, value) in self.m.iter().zip(out.iter_mut()) {
            *value += row[0] * p[0] + row[1] * p[1] + row[2] * p[2];
        }
        out
    }

    fn compose(&self, child: &Transform) -> Transform {
        let mut m = [[0.0; 3]; 3];
        for (row, self_row) in m.iter_mut().zip(&self.m) {
            for (j, value) in row.iter_mut().enumerate() {
                *value = (0..3).map(|k| self_row[k] * child.m[k][j]).sum();
            }
        }
        Transform {
            m,
            t: self.apply(child.t),
        }
    }
}

struct Loader {
    /// Documents embedded in an MPD file, keyed by normalized name.
    embedded_docs: HashMap<String, Rc<Vec<Command>>>,
    /// Files already parsed from disk; `None` records a failed lookup.
    file_cache: HashMap<String, Option<Rc<Vec<Command>>>>,
    colors: HashMap<u32, [u8; 4]>,
    warned_colors: HashSet<u32>,
    search_dirs: Vec<PathBuf>,
    /// Colour -> flat vertex positions (nine floats per triangle).
    triangles: BTreeMap<[u8; 4], Vec<f32>>,
    missing_subfiles: BTreeSet<String>,
    logger: Logger,
}

impl Loader {
    fn new(search_dirs: Vec<PathBuf>, logger: Logger) -> Self {
        Self {
            embedded_docs: HashMap::new(),
            file_cache: HashMap::new(),
            colors: CORE_COLORS
                .iter()
                .map(|&(code, rgb, alpha)| {
                    (code, [(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8, alpha])
                })
                .collect(),
            warned_colors: HashSet::new(),
            search_dirs,
            triangles: BTreeMap::new(),
            missing_subfiles: BTreeSet::new(),
            logger,
        }
    }

    /// Overrides the built-in colour table with an installed LDConfig.ldr.
    fn load_ldconfig(&mut self, path: &Path) {
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        for command in parse_commands(&text) {
            if let Command::ColourDef { code, rgba } = command {
                self.colors.insert(code, rgba);
            }
        }
    }

    fn load(mut self, text: &str) -> ConversionResult<LDrawModel> {
        let documents = split_mpd(text);
        let main = Rc::new(parse_commands(documents[0].1));
        for (name, body) in &documents[1..] {
            self.embedded_docs
                .insert(normalize_name(name), Rc::new(parse_commands(body)));
        }

        let mut visiting = Vec::new();
        self.walk(&main, Transform::IDENTITY, FALLBACK_COLOR, 0, &mut visiting);

        if self.triangles.values().all(|positions| positions.is_empty()) {
            return Err(ConversionError::ObjParseError(
                "LDraw file contains no triangle or quad geometry to voxelize".to_string(),
            ));
        }

        let mut models = Vec::new();
        let mut material_images = Vec::new();
        for (material_id, (rgba, positions)) in self.triangles.into_iter().enumerate() {
            let vertex_count = positions.len() / 3;
            let mesh = tobj::Mesh {
                positions,
                indices: (0..vertex_count as u32).collect(),
                material_id: Some(material_id),
                ..Default::default()
            };
            models.push(tobj::Model::new(mesh, format!("ldraw_color_{material_id}")));
            material_images.push(image::RgbaImage::from_pixel(1, 1, image::Rgba(rgba)));
        }

        Ok(LDrawModel {
            models,
            material_images,
            missing_subfiles: self.missing_subfiles.into_iter().collect(),
        })
    }

    fn walk(
        &mut self,
        commands: &[Command],
        transform: Transform,
        color: [u8; 4],
        depth: usize,
        visiting: &mut Vec<String>,
    ) {
        for command in commands {
            match command {
                Command::ColourDef { code, rgba } => {
                    self.colors.insert(*code, *rgba);
                }
                Command::Triangle {
                    color: code,
                    vertices,
                } => {
                    let rgba = self.resolve_color(*code, color);
                    self.emit_triangle(&transform, rgba, vertices);
                }
                Command::Quad {
                    color: code,
                    vertices,
                } => {
                    let rgba = self.resolve_color(*code, color);
                    self.emit_triangle(&transform, rgba, &[vertices[0], vertices[1], vertices[2]]);
                    self.emit_triangle(&transform, rgba, &[vertices[0], vertices[2], vertices[3]]);
                }
                Command::SubFile {
                    color: code,
                    transform: child,
                    name,
                } => {
                    if depth >= MAX_RECURSION_DEPTH {
                        self.logger.log(format!(
                            "  Skipping {name}: exceeds max subfile depth of {MAX_RECURSION_DEPTH}"
                        ));
                        continue;
                    }
                    let normalized = normalize_name(name);
                    if visiting.contains(&normalized) {
                        self.logger
                            .log(format!("  Skipping {name}: circular subfile reference"));
                        continue;
                    }
                    let Some(commands) = self.resolve_subfile(name, &normalized) else {
                        if self.missing_subfiles.insert(name.clone()) {
                            self.logger.log(format!("  Missing subfile: {name}"));
                        }
                        continue;
                    };
                    let rgba = self.resolve_color(*code, color);
                    visiting.push(normalized);
                    self.walk(&commands, transform.compose(child), rgba, depth + 1, visiting);
                    visiting.pop();
                }
            }
        }
    }

    fn emit_triangle(&mut self, transform: &Transform, rgba: [u8; 4], vertices: &[[f32; 3]; 3]) {
        let positions = self.triangles.entry(rgba).or_default();
        for vertex in vertices {
            let [x, y, z] = transform.apply(*vertex);
            // LDraw is -Y up in LDU; rotate 180 degrees about X into the Y-up
            // space the voxelizer expects and convert LDU to studs.
            positions.push(x / LDU_PER_STUD);
            positions.push(-y / LDU_PER_STUD);
            positions.push(-z / LDU_PER_STUD);
        }
    }

    fn resolve_color(&mut self, code: u32, inherited: [u8; 4]) -> [u8; 4] {
        if code == CODE_INHERIT || code == CODE_EDGE_INHERIT {
            return inherited;
        }
        if let Some(&rgba) = self.colors.get(&code) {
            return rgba;
        }
        // Direct colours encode an opaque RGB value as 0x2RRGGBB.
        if (0x2000000..0x3000000).contains(&code) {
            return [(code >> 16) as u8, (code >> 8) as u8, code as u8, 255];
        }
        if self.warned_colors.insert(code) {
            self.logger
                .log(format!("  Unknown LDraw colour {code}, using fallback grey"));
        }
        FALLBACK_COLOR
    }

    fn resolve_subfile(&mut self, name: &str, normalized: &str) -> Option<Rc<Vec<Command>>> {
        if let Some(doc) = self.embedded_docs.get(normalized) {
            return Some(doc.clone());
        }
        if let Some(cached) = self.file_cache.get(normalized) {
            return cached.clone();
        }

        // Official part references are conventionally lowercase, but some
        // files reference mixed-case names; on case-sensitive filesystems
        // try the reference as written too.
        let as_written = name.trim().replace('\\', "/");
        let mut found = None;
        'search: for dir in &self.search_dirs {
            for relative in [normalized, as_written.as_str()] {
                let candidate = dir.join(relative);
                if candidate.is_file() {
                    if let Ok(text) = std::fs::read_to_string(&candidate) {
                        found = Some(Rc::new(parse_commands(&text)));
                        break 'search;
                    }
                }
            }
        }

        self.file_cache.insert(normalized.to_string(), found.clone());
        found
    }
}

/// Normalizes a subfile reference: LDraw uses backslash separators and
/// case-insensitive names.
fn normalize_name(name: &str) -> String {
    name.trim().replace('\\', "/").to_ascii_lowercase()
}

fn is_library_root(dir: &Path) -> bool {
    dir.join("LDConfig.ldr").is_file()
        || (dir.join("parts").is_dir() && dir.join("p").is_dir())
}

/// Directories that may hold an LDraw parts library, most specific first.
fn library_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for var in ["LDRAWDIR", "LDRAW_DIR"] {
        if let Ok(dir) = std::env::var(var) {
            if !dir.is_empty() {
                roots.push(PathBuf::from(dir));
            }
        }
    }
    for var in ["HOME", "USERPROFILE"] {
        if let Ok(home) = std::env::var(var) {
            if !home.is_empty() {
                roots.push(PathBuf::from(home).join(".ldraw"));
            }
        }
    }
    roots.push(PathBuf::from("/usr/share/ldraw"));
    roots.push(PathBuf::from("/usr/local/share/ldraw"));
    roots.push(PathBuf::from("C:/LDraw"));
    roots.retain(|root| root.is_dir());
    roots
}

/// Splits an MPD file into `(name, body)` documents on `0 FILE` lines. The
/// first entry is the main document; plain (non-MPD) files yield exactly one.
fn split_mpd(text: &str) -> Vec<(&str, &str)> {
    let mut documents: Vec<(&str, usize, usize)> = vec![("", 0, text.len())];
    for (offset, line) in line_offsets(text) {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix('0') {
            let rest = rest.trim_start();
            if let Some(name) = rest
                .strip_prefix("FILE ")
                .or_else(|| rest.strip_prefix("FILE\t"))
            {
                if documents.len() == 1 && text[..offset].trim().is_empty() {
                    // `0 FILE` opens the very first document: the implicit
                    // main document would be empty, so drop it.
                    documents.clear();
                }
                if let Some(last) = documents.last_mut() {
                    last.2 = offset;
                }
                documents.push((name.trim(), offset, text.len()));
            }
        }
    }
    documents
        .into_iter()
        .map(|(name, start, end)| (name, &text[start..end]))
        .collect()
}

fn line_offsets(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.lines().scan(0usize, |offset, line| {
        let start = *offset;
        *offset = start + line.len() + 1;
        Some((start, line))
    })
}

/// Parses LDraw text into commands, silently skipping malformed lines: LDraw
/// files in the wild routinely contain stray meta lines and comments.
fn parse_commands(text: &str) -> Vec<Command> {
    let mut commands = Vec::new();
    for line in text.lines() {
        let Some((line_type, rest)) = next_token(line) else {
            continue;
        };
        match line_type {
            "0" => {
                if let Some(colour) = parse_colour_def(rest) {
                    commands.push(colour);
                }
            }
            "1" => {
                let Some((color, rest)) = parse_color_code(rest) else {
                    continue;
                };
                let Some((values, rest)) = parse_floats::<12>(rest) else {
                    continue;
                };
                let name = rest.trim();
                if name.is_empty() {
                    continue;
                }
                commands.push(Command::SubFile {
                    color,
                    transform: Transform {
                        m: [
                            [values[3], values[4], values[5]],
                            [values[6], values[7], values[8]],
                            [values[9], values[10], values[11]],
                        ],
                        t: [values[0], values[1], values[2]],
                    },
                    name: name.to_string(),
                });
            }
            "3" => {
                let Some((color, rest)) = parse_color_code(rest) else {
                    continue;
                };
                let Some((v, _)) = parse_floats::<9>(rest) else {
                    continue;
                };
                commands.push(Command::Triangle {
                    color,
                    vertices: [[v[0], v[1], v[2]], [v[3], v[4], v[5]], [v[6], v[7], v[8]]],
                });
            }
            "4" => {
                let Some((color, rest)) = parse_color_code(rest) else {
                    continue;
                };
                let Some((v, _)) = parse_floats::<12>(rest) else {
                    continue;
                };
                commands.push(Command::Quad {
                    color,
                    vertices: [
                        [v[0], v[1], v[2]],
                        [v[3], v[4], v[5]],
                        [v[6], v[7], v[8]],
                        [v[9], v[10], v[11]],
                    ],
                });
            }
            _ => {}
        }
    }
    commands
}

/// Parses a `!COLOUR <name> CODE <code> VALUE #RRGGBB ... [ALPHA <a>]` meta
/// line (the format used by LDConfig.ldr and inline colour definitions).
fn parse_colour_def(rest: &str) -> Option<Command> {
    let (keyword, mut rest) = next_token(rest)?;
    if !keyword.eq_ignore_ascii_case("!COLOUR") {
        return None;
    }

    let mut code = None;
    let mut value = None;
    let mut alpha = 255u8;
    while let Some((token, after)) = next_token(rest) {
        rest = after;
        let Some((argument, after)) = next_token(rest) else {
            break;
        };
        if token.eq_ignore_ascii_case("CODE") {
            code = argument.parse::<u32>().ok();
            rest = after;
        } else if token.eq_ignore_ascii_case("VALUE") {
            value = parse_hex_color(argument);
            rest = after;
        } else if token.eq_ignore_ascii_case("ALPHA") {
            alpha = argument.parse::<u8>().unwrap_or(255);
            rest = after;
        }
    }

    let code = code?;
    let [r, g, b] = value?;
    Some(Command::ColourDef {
        code,
        rgba: [r, g, b, alpha],
    })
}

fn parse_hex_color(token: &str) -> Option<[u8; 3]> {
    let hex = token.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let value = u32::from_str_radix(hex, 16).ok()?;
    Some([(value >> 16) as u8, (value >> 8) as u8, value as u8])
}

/// Colour codes may be decimal or hex (`0x2RRGGBB` direct colours).
fn parse_color_code(rest: &str) -> Option<(u32, &str)> {
    let (token, rest) = next_token(rest)?;
    let code = if let Some(hex) = token.strip_prefix("0x").or_else(|| token.strip_prefix("0X")) {
        u32::from_str_radix(hex, 16).ok()?
    } else {
        token.parse().ok()?
    };
    Some((code, rest))
}

fn parse_floats<const N: usize>(mut rest: &str) -> Option<([f32; N], &str)> {
    let mut values = [0.0; N];
    for value in &mut values {
        let (token, after) = next_token(rest)?;
        *value = token.parse().ok()?;
        rest = after;
    }
    Some((values, rest))
}

fn next_token(s: &str) -> Option<(&str, &str)> {
    let s = s.trim_start();
    if s.is_empty() {
        return None;
    }
    let end = s.find(char::is_whitespace).unwrap_or(s.len());
    Some((&s[..end], &s[end..]))
}

/// Built-in subset of the standard LDraw colour table `(code, 0xRRGGBB,
/// alpha)`. Values follow LDConfig.ldr; an installed LDConfig.ldr overrides
/// this table, and inline `0 !COLOUR` definitions override both.
const CORE_COLORS: &[(u32, u32, u8)] = &[
    (0, 0x05131D, 255),   // Black
    (1, 0x0055BF, 255),   // Blue
    (2, 0x237841, 255),   // Green
    (3, 0x008F9B, 255),   // Dark Turquoise
    (4, 0xC91A09, 255),   // Red
    (5, 0xC870A0, 255),   // Dark Pink
    (6, 0x583927, 255),   // Brown
    (7, 0x9BA19D, 255),   // Light Gray
    (8, 0x6D6E5C, 255),   // Dark Gray
    (9, 0xB4D2E3, 255),   // Light Blue
    (10, 0x4B9F4A, 255),  // Bright Green
    (11, 0x55A5AF, 255),  // Light Turquoise
    (12, 0xF2705E, 255),  // Salmon
    (13, 0xFC97AC, 255),  // Pink
    (14, 0xF2CD37, 255),  // Yellow
    (15, 0xFFFFFF, 255),  // White
    (17, 0xC2DAB8, 255),  // Light Green
    (18, 0xFBE696, 255),  // Light Yellow
    (19, 0xE4CD9E, 255),  // Tan
    (20, 0xC9CAE2, 255),  // Light Violet
    (22, 0x81007B, 255),  // Purple
    (23, 0x2032B0, 255),  // Dark Blue Violet
    (25, 0xFE8A18, 255),  // Orange
    (26, 0x923978, 255),  // Magenta
    (27, 0xBBE90B, 255),  // Lime
    (28, 0x958A73, 255),  // Dark Tan
    (29, 0xE4ADC8, 255),  // Bright Pink
    (30, 0xAC78BA, 255),  // Medium Lavender
    (31, 0xE1D5ED, 255),  // Lavender
    (33, 0x0020A0, 128),  // Trans Dark Blue
    (34, 0x237841, 128),  // Trans Green
    (35, 0x56E646, 128),  // Trans Bright Green
    (36, 0xC91A09, 128),  // Trans Red
    (37, 0x672F99, 128),  // Trans Purple
    (38, 0xFF800D, 128),  // Trans Neon Orange
    (39, 0xC1DFF0, 128),  // Trans Very Light Blue
    (40, 0x635F52, 128),  // Trans Black
    (41, 0x559AB7, 128),  // Trans Medium Blue
    (42, 0xC0FF00, 128),  // Trans Neon Green
    (43, 0xAEE9EF, 128),  // Trans Light Blue
    (44, 0x96709F, 128),  // Trans Bright Reddish Lilac
    (45, 0xFC97AC, 128),  // Trans Pink
    (46, 0xF5CD2F, 128),  // Trans Yellow
    (47, 0xFCFCFC, 128),  // Trans Clear
    (54, 0xDAB000, 128),  // Trans Neon Yellow
    (57, 0xF08F1C, 128),  // Trans Orange
    (68, 0xF3CF9B, 255),  // Very Light Orange
    (69, 0xCD6298, 255),  // Bright Reddish Lilac
    (70, 0x582A12, 255),  // Reddish Brown
    (71, 0xA0A5A9, 255),  // Light Bluish Gray
    (72, 0x6C6E68, 255),  // Dark Bluish Gray
    (73, 0x5A93DB, 255),  // Medium Blue
    (74, 0x73DCA1, 255),  // Medium Green
    (77, 0xFECCCF, 255),  // Light Pink
    (78, 0xF6D7B3, 255),  // Light Nougat
    (80, 0xA5A9B4, 255),  // Metallic Silver
    (81, 0x899B5F, 255),  // Metallic Green
    (82, 0xDBAC34, 255),  // Metallic Gold
    (83, 0x1A2831, 255),  // Metallic Black
    (84, 0xCC702A, 255),  // Medium Nougat
    (85, 0x3F3691, 255),  // Medium Lilac
    (86, 0x7C503A, 255),  // Light Brown
    (89, 0x4C61DB, 255),  // Blue Violet
    (92, 0xD09168, 255),  // Nougat
    (100, 0xFEBABD, 255), // Light Salmon
    (110, 0x4354A3, 255), // Violet
    (112, 0x6874CA, 255), // Medium Violet
    (115, 0xC7D23C, 255), // Medium Lime
    (118, 0xB3D7D1, 255), // Aqua
    (120, 0xD9E4A7, 255), // Light Lime
    (125, 0xF9BA61, 255), // Light Orange
    (135, 0x9CA3A8, 255), // Pearl Light Gray
    (142, 0xDCBC81, 255), // Pearl Light Gold
    (148, 0x575857, 255), // Pearl Dark Gray
    (151, 0xE6E3E0, 255), // Very Light Bluish Gray
    (178, 0xB48455, 255), // Flat Dark Gold
    (179, 0x898788, 255), // Flat Silver
    (183, 0xF2F3F2, 255), // Pearl White
    (191, 0xF8BB3D, 255), // Bright Light Orange
    (212, 0x86C1E1, 255), // Bright Light Blue
    (216, 0xB31004, 255), // Rust
    (226, 0xFFF03A, 255), // Bright Light Yellow
    (232, 0x56BED6, 255), // Sky Blue
    (256, 0x212121, 255), // Rubber Black
    (272, 0x0D325B, 255), // Dark Blue
    (288, 0x184632, 255), // Dark Green
    (297, 0xAA7F2E, 255), // Pearl Gold
    (308, 0x352100, 255), // Dark Brown
    (313, 0x54A9C8, 255), // Maersk Blue
    (320, 0x720E0F, 255), // Dark Red
    (321, 0x1498D7, 255), // Dark Azure
    (322, 0x3EC2DD, 255), // Medium Azure
    (323, 0xBDDCD8, 255), // Light Aqua
    (326, 0xDFEEA5, 255), // Yellowish Green
    (330, 0x9B9A5A, 255), // Olive Green
    (334, 0xBBA53D, 255), // Chrome Gold
    (335, 0xD67572, 255), // Sand Red
    (351, 0xF785B1, 255), // Medium Dark Pink
    (353, 0xFF6D77, 255), // Coral
    (366, 0xFA9C1C, 255), // Earth Orange
    (373, 0x845E84, 255), // Sand Purple
    (378, 0xA0BCAC, 255), // Sand Green
    (379, 0x597184, 255), // Sand Blue
    (383, 0xE0E0E0, 255), // Chrome Silver
    (450, 0xB67B50, 255), // Fabuland Brown
    (462, 0xFFA70B, 255), // Medium Orange
    (484, 0xA95500, 255), // Dark Orange
    (503, 0xE6E3DA, 255), // Very Light Gray
];

#[cfg(test)]
mod tests {
    use super::*;

    fn silent() -> Logger {
        Logger::new()
    }

    /// 40x40x24 LDU box (2x2 brick footprint) out of quads, in red.
    const BOX_DAT: &str = "\
0 test box
4 4 -20 -24 -20 20 -24 -20 20 -24 20 -20 -24 20
4 4 -20 0 -20 20 0 -20 20 0 20 -20 0 20
4 4 -20 -24 -20 20 -24 -20 20 0 -20 -20 0 -20
4 4 -20 -24 20 20 -24 20 20 0 20 -20 0 20
4 4 -20 -24 -20 -20 -24 20 -20 0 20 -20 0 -20
4 4 20 -24 -20 20 -24 20 20 0 20 20 0 -20
";

    fn bounds(models: &[tobj::Model]) -> ([f32; 3], [f32; 3]) {
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for model in models {
            for vertex in model.mesh.positions.chunks_exact(3) {
                for axis in 0..3 {
                    min[axis] = min[axis].min(vertex[axis]);
                    max[axis] = max[axis].max(vertex[axis]);
                }
            }
        }
        (min, max)
    }

    #[test]
    fn parses_quads_and_converts_units_and_axes() {
        let loaded = load_ldraw_bytes(BOX_DAT.as_bytes(), &silent()).unwrap();
        assert_eq!(loaded.models.len(), 1);
        assert!(loaded.missing_subfiles.is_empty());

        // 6 quads = 12 triangles = 36 vertices.
        assert_eq!(loaded.models[0].mesh.positions.len(), 36 * 3);

        // 40 LDU wide = 2 studs; -24..0 LDU tall (Y-down) = 0..1.2 studs up.
        let (min, max) = bounds(&loaded.models);
        assert_eq!(min, [-1.0, 0.0, -1.0]);
        assert_eq!(max, [1.0, 1.2, 1.0]);

        // Colour 4 = red.
        assert_eq!(
            *loaded.material_images[0].get_pixel(0, 0),
            image::Rgba([0xC9, 0x1A, 0x09, 255])
        );
    }

    #[test]
    fn triangles_group_by_colour() {
        let dat = "\
3 4 0 0 0 1 0 0 0 1 0
3 2 0 0 0 1 0 0 0 0 1
3 4 0 0 0 0 1 0 0 0 1
";
        let loaded = load_ldraw_bytes(dat.as_bytes(), &silent()).unwrap();
        assert_eq!(loaded.models.len(), 2);
        let triangle_counts: Vec<usize> = loaded
            .models
            .iter()
            .map(|m| m.mesh.positions.len() / 9)
            .collect();
        let mut sorted = triangle_counts.clone();
        sorted.sort();
        assert_eq!(sorted, vec![1, 2]);
        for (id, model) in loaded.models.iter().enumerate() {
            assert_eq!(model.mesh.material_id, Some(id));
        }
    }

    #[test]
    fn resolves_embedded_mpd_subfiles_with_transform_and_colour() {
        let mpd = "\
0 FILE main.ldr
1 2 0 0 0 2 0 0 0 2 0 0 0 2 part.dat
0 FILE part.dat
3 16 0 0 0 10 0 0 0 10 0
";
        let loaded = load_ldraw_bytes(mpd.as_bytes(), &silent()).unwrap();
        assert!(loaded.missing_subfiles.is_empty());
        assert_eq!(loaded.models.len(), 1);

        // Subfile scaled 2x: 10 LDU legs become 20 LDU = 1 stud.
        let (min, max) = bounds(&loaded.models);
        assert_eq!(min[0], 0.0);
        assert_eq!(max[0], 1.0);

        // Colour 16 inherits the reference's colour 2 (green).
        assert_eq!(
            *loaded.material_images[0].get_pixel(0, 0),
            image::Rgba([0x23, 0x78, 0x41, 255])
        );
    }

    #[test]
    fn records_missing_subfiles_and_keeps_going() {
        let dat = "\
1 16 0 0 0 1 0 0 0 1 0 0 0 1 no-such-part.dat
3 4 0 0 0 1 0 0 0 1 0
";
        let loaded = load_ldraw_bytes(dat.as_bytes(), &silent()).unwrap();
        assert_eq!(loaded.missing_subfiles, vec!["no-such-part.dat".to_string()]);
        assert_eq!(loaded.models.len(), 1);
    }

    #[test]
    fn inline_colour_definitions_and_direct_colours() {
        let dat = "\
0 !COLOUR My_Colour CODE 500 VALUE #102030 ALPHA 128
3 500 0 0 0 1 0 0 0 1 0
3 0x2FF8000 0 0 0 1 0 0 0 0 1
";
        let loaded = load_ldraw_bytes(dat.as_bytes(), &silent()).unwrap();
        let pixels: Vec<image::Rgba<u8>> = loaded
            .material_images
            .iter()
            .map(|img| *img.get_pixel(0, 0))
            .collect();
        assert!(pixels.contains(&image::Rgba([0x10, 0x20, 0x30, 128])));
        assert!(pixels.contains(&image::Rgba([0xFF, 0x80, 0x00, 255])));
    }

    #[test]
    fn circular_references_do_not_hang() {
        let mpd = "\
0 FILE a.ldr
1 16 0 0 0 1 0 0 0 1 0 0 0 1 b.ldr
3 4 0 0 0 1 0 0 0 1 0
0 FILE b.ldr
1 16 0 0 0 1 0 0 0 1 0 0 0 1 a.ldr
";
        let loaded = load_ldraw_bytes(mpd.as_bytes(), &silent()).unwrap();
        assert_eq!(loaded.models.len(), 1);
    }

    #[test]
    fn empty_geometry_is_an_error() {
        let dat = "0 just a comment\n2 24 0 0 0 1 1 1\n";
        assert!(load_ldraw_bytes(dat.as_bytes(), &silent()).is_err());
    }

    #[test]
    fn sniffs_ldraw_vs_obj() {
        assert!(looks_like_ldraw(BOX_DAT.as_bytes()));
        assert!(!looks_like_ldraw(
            b"# comment\nv 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n"
        ));
    }

    #[test]
    fn resolves_primitives_when_opening_a_file_inside_a_library() {
        let root = std::env::temp_dir().join(format!(
            "obj2brz-ldraw-lib-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        std::fs::create_dir_all(root.join("parts")).unwrap();
        std::fs::create_dir_all(root.join("p/48")).unwrap();
        std::fs::write(
            root.join("parts/test.dat"),
            "1 4 0 0 0 1 0 0 0 1 0 0 0 1 prim.dat\n\
             1 4 0 0 0 1 0 0 0 1 0 0 0 1 48\\edge.dat\n",
        )
        .unwrap();
        std::fs::write(root.join("p/prim.dat"), "3 16 0 0 0 1 0 0 0 1 0\n").unwrap();
        std::fs::write(root.join("p/48/edge.dat"), "3 16 0 0 0 1 0 0 0 0 1\n").unwrap();

        let loaded = load_ldraw(&root.join("parts/test.dat"), &silent()).unwrap();
        assert_eq!(loaded.missing_subfiles, Vec::<String>::new());
        let triangles: usize = loaded
            .models
            .iter()
            .map(|m| m.mesh.positions.len() / 9)
            .sum();
        assert_eq!(triangles, 2);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn detects_ldraw_extensions() {
        assert!(is_ldraw_path("model.dat"));
        assert!(is_ldraw_path("Model.LDR"));
        assert!(is_ldraw_path("scene.mpd"));
        assert!(!is_ldraw_path("model.obj"));
        assert!(!is_ldraw_path("model"));
    }
}
