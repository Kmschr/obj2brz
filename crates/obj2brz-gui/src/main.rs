mod gui;

use eframe::{egui, egui::*, App};
#[cfg(not(target_arch = "wasm32"))]
use eframe::{run_native, NativeOptions};
use gui::bool_color;
use obj2brz::{
    convert, model_bounds, validate_obj_resources, BrickType, ConvertOptions, Logger, Material,
    ModelBounds, OutputFormat,
};
#[cfg(not(target_arch = "wasm32"))]
use rfd::FileDialog;
use rfd::{MessageDialog, MessageLevel};
use serde::{Deserialize, Serialize};
#[cfg(not(target_arch = "wasm32"))]
use std::env;
use std::{path::Path, path::PathBuf, sync::mpsc, sync::mpsc::Receiver, thread};
use uuid::Uuid;

#[cfg(not(target_arch = "wasm32"))]
const WINDOW_WIDTH: f32 = 1200.;
#[cfg(not(target_arch = "wasm32"))]
const WINDOW_HEIGHT: f32 = 860.;

/// GUI application state. Wraps the UI-agnostic [`ConvertOptions`] with the
/// transient widgets and channels the egui front-end needs.
#[derive(Debug, Serialize, Deserialize)]
pub struct Obj2Brs {
    pub bricktype: BrickType,
    pub brick_scale: isize,
    #[serde(skip)]
    input_file_path_receiver: Option<Receiver<Option<PathBuf>>>,
    input_file_path: String,
    #[serde(skip)]
    model_bounds_receiver: Option<Receiver<(String, Result<ModelBounds, String>)>>,
    #[serde(skip)]
    model_bounds_path: String,
    #[serde(skip)]
    model_bounds: Option<Result<ModelBounds, String>>,
    material: Material,
    material_intensity: u32,
    #[serde(default = "default_player_collision")]
    player_collision: bool,
    #[serde(default = "default_physics_collision")]
    physics_collision: bool,
    #[serde(skip)]
    output_directory_receiver: Option<Receiver<Option<PathBuf>>>,
    output_directory: String,
    copy_to_clipboard: bool,
    output_format: OutputFormat,
    save_owner_id: String,
    save_owner_name: String,
    save_name: String,
    scale: f32,
    simplify: bool,
    #[serde(default)]
    rampify: bool,
    split_by_material: bool,
    grid_offset_x: f32,
    grid_offset_y: f32,
    grid_offset_z: f32,
    #[serde(skip)]
    missing_resources_dialog: Option<String>,
    #[serde(skip)]
    pending_conversion_skip_textures: bool,
    #[serde(skip)]
    logger: Logger,
    #[serde(skip)]
    conversion_in_progress: bool,
    #[serde(skip)]
    conversion_done_receiver: Option<Receiver<()>>,
    #[serde(default = "default_dark_mode")]
    dark_mode: bool,
}

fn default_dark_mode() -> bool {
    true
}

fn default_player_collision() -> bool {
    true
}

fn default_physics_collision() -> bool {
    true
}

fn format_studs(value: f32) -> String {
    let formatted = format!("{value:.1}");
    formatted.trim_end_matches('0').trim_end_matches('.').to_string()
}

impl Default for Obj2Brs {
    fn default() -> Self {
        Self {
            bricktype: BrickType::Microbricks,
            brick_scale: 1,
            input_file_path_receiver: None,
            input_file_path: "test.obj".into(),
            model_bounds_receiver: None,
            model_bounds_path: String::new(),
            model_bounds: None,
            material: Material::Plastic,
            material_intensity: 5,
            player_collision: true,
            physics_collision: true,
            output_directory_receiver: None,
            output_directory: "builds".into(),
            copy_to_clipboard: false,
            output_format: OutputFormat::Brz,
            save_owner_id: "d66c4ad5-59fc-4a9b-80b8-08dedc25bff9".into(),
            save_owner_name: "obj2brz".into(),
            save_name: "test".into(),
            scale: 1.0,
            simplify: false,
            rampify: false,
            split_by_material: false,
            grid_offset_x: 0.0,
            grid_offset_y: 0.0,
            grid_offset_z: 0.0,
            missing_resources_dialog: None,
            pending_conversion_skip_textures: false,
            logger: Logger::new(),
            conversion_in_progress: false,
            conversion_done_receiver: None,
            dark_mode: true,
        }
    }
}

impl Obj2Brs {
    /// Builds the UI-agnostic conversion options from the current app state.
    fn to_options(&self) -> ConvertOptions {
        ConvertOptions {
            bricktype: self.bricktype,
            brick_scale: self.brick_scale,
            input_file_path: self.input_file_path.clone(),
            material: self.material,
            material_intensity: self.material_intensity,
            player_collision: self.player_collision,
            physics_collision: self.physics_collision,
            output_directory: self.output_directory.clone(),
            copy_to_clipboard: self.copy_to_clipboard,
            output_format: self.output_format,
            save_owner_id: self.save_owner_id.clone(),
            save_owner_name: self.save_owner_name.clone(),
            save_name: self.save_name.clone(),
            scale: self.scale,
            simplify: self.simplify,
            rampify: self.rampify,
            split_by_material: self.split_by_material,
            grid_offset_x: self.grid_offset_x,
            grid_offset_y: self.grid_offset_y,
            grid_offset_z: self.grid_offset_z,
            logger: self.logger.clone(),
        }
    }
}

impl App for Obj2Brs {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, eframe::APP_KEY, self);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        let background = if self.dark_mode {
            Color32::from_rgb(30, 41, 56)
        } else {
            Color32::from_rgb(244, 247, 251)
        };
        background.to_normalized_gamma_f32()
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Request repaint to keep updating logs
        ctx.request_repaint();

        gui::configure_style(ctx, self.dark_mode);
        self.receive_file_dialog_messages();
        self.refresh_model_bounds();

        let input_file_valid = Path::new(&self.input_file_path).exists();
        let output_dir_valid = Path::new(&self.output_directory).is_dir();
        let uuid_valid = Uuid::parse_str(&self.save_owner_id).is_ok();
        let settings_error = self.to_options().settings_error();
        let can_convert = input_file_valid
            && output_dir_valid
            && uuid_valid
            && settings_error.is_none()
            && !self.conversion_in_progress;

        // Show missing resources dialog if needed
        self.show_missing_resources_dialog(ctx);

        gui::header(ctx, &mut self.dark_mode);
        gui::tools_sidebar(ctx);
        self.bottom_dock(
            ctx,
            can_convert,
            input_file_valid,
            output_dir_valid,
            uuid_valid,
            settings_error.as_deref(),
        );

        CentralPanel::default()
            .frame(Frame::default().fill(ctx.style().visuals.panel_fill))
            .show(ctx, |ui: &mut Ui| {
                ScrollArea::vertical().show(ui, |ui| {
                    ui.add_space(4.0);
                    gui::card(ui, "SOURCE MODEL", |ui| {
                        self.source_card(ui, input_file_valid);
                    });
                    gui::card(ui, "OUTPUT", |ui| {
                        self.output_card(ui, output_dir_valid);
                    });
                    gui::card(ui, "BRICKS", |ui| {
                        self.bricks_card(ui);
                    });

                    egui::CollapsingHeader::new(RichText::new("Advanced options").strong())
                        .default_open(false)
                        .show(ui, |ui| {
                            ui.add_space(4.0);
                            gui::form_grid(ui, "advanced_grid", |ui| {
                                self.advanced_options(ui, uuid_valid);
                            });
                            ui.add_space(8.0);
                            gui::info_text(ui);
                        });
                    ui.add_space(8.0);
                });
            });
    }
}

impl Obj2Brs {
    fn receive_file_dialog_messages(&mut self) {
        if let Some(rx) = &self.input_file_path_receiver {
            if let Ok(data) = rx.try_recv() {
                self.input_file_path_receiver = None;
                if let Some(path) = data {
                    if let Ok(path_str) = path.into_os_string().into_string() {
                        self.input_file_path = path_str;
                    }
                }
            }
        }

        if let Some(rx) = &self.output_directory_receiver {
            if let Ok(data) = rx.try_recv() {
                self.output_directory_receiver = None;
                if let Some(path) = data {
                    if let Ok(path_str) = path.into_os_string().into_string() {
                        self.output_directory = path_str;
                    }
                }
            }
        }

        // Check if conversion is done
        if let Some(rx) = &self.conversion_done_receiver {
            if rx.try_recv().is_ok() {
                self.conversion_done_receiver = None;
                self.conversion_in_progress = false;
            }
        }
    }

    /// Loads bounds away from the UI thread whenever the selected path changes.
    fn refresh_model_bounds(&mut self) {
        if let Some(rx) = &self.model_bounds_receiver {
            if let Ok((path, result)) = rx.try_recv() {
                self.model_bounds_receiver = None;
                if path == self.input_file_path {
                    self.model_bounds_path = path;
                    self.model_bounds = Some(result);
                }
            }
        }

        if self.model_bounds_receiver.is_none() && self.model_bounds_path != self.input_file_path {
            let path = self.input_file_path.clone();
            self.model_bounds_path = path.clone();
            self.model_bounds = None;
            let (tx, rx) = mpsc::channel();
            self.model_bounds_receiver = Some(rx);
            thread::spawn(move || {
                let result = model_bounds(&path).map_err(|error| error.to_string());
                let _ = tx.send((path, result));
            });
        }
    }

    /// Resizable bottom dock containing conversion controls and the log stream.
    fn bottom_dock(
        &mut self,
        ctx: &egui::Context,
        can_convert: bool,
        input_file_valid: bool,
        output_dir_valid: bool,
        uuid_valid: bool,
        settings_error: Option<&str>,
    ) {
        TopBottomPanel::bottom("bottom_dock")
            .resizable(true)
            .default_height(300.0)
            .height_range(200.0..=500.0)
            .frame(
                Frame::default()
                    .fill(ctx.style().visuals.panel_fill)
                    .stroke(ctx.style().visuals.window_stroke)
                    .inner_margin(egui::Margin::symmetric(16, 10)),
            )
            .show(ctx, |ui| {
                self.action_bar_contents(
                    ui,
                    can_convert,
                    input_file_valid,
                    output_dir_valid,
                    uuid_valid,
                    settings_error,
                );
                ui.add_space(4.0);
                ui.separator();
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("CONSOLE")
                            .strong()
                            .size(13.0)
                            .color(ui.visuals().weak_text_color()),
                    );
                    if self.conversion_in_progress {
                        ui.spinner();
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button("Clear").clicked() {
                            self.logger.clear();
                        }
                    });
                });
                ui.add_space(6.0);

                let log_height = ui.available_height();
                Frame::default()
                    .fill(ui.visuals().extreme_bg_color)
                    .corner_radius(egui::CornerRadius::same(6))
                    .inner_margin(10.0)
                    .show(ui, |ui| {
                        ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .max_height(log_height - 20.0)
                            .stick_to_bottom(true)
                            .show(ui, |ui| {
                                let messages = self.logger.get_messages();
                                if messages.is_empty() {
                                    ui.label(
                                        RichText::new("Waiting for conversion…")
                                            .color(ui.visuals().weak_text_color())
                                            .monospace(),
                                    );
                                } else {
                                    for message in messages {
                                        gui::log_line(ui, &message);
                                    }
                                }
                            });
                    });
            });
    }

    /// Conversion controls and live validation hints above the console.
    fn action_bar_contents(
        &mut self,
        ui: &mut Ui,
        can_convert: bool,
        input_file_valid: bool,
        output_dir_valid: bool,
        uuid_valid: bool,
        settings_error: Option<&str>,
    ) {
        let mut issues: Vec<String> = Vec::new();
        if !input_file_valid {
            issues.push("Model file not found".into());
        }
        if !output_dir_valid {
            issues.push("Output directory doesn't exist".into());
        }
        if !uuid_valid {
            issues.push("Brick owner ID is not a valid UUID".into());
        }
        if let Some(err) = settings_error {
            issues.push(err.to_string());
        }

        ui.horizontal(|ui| {
            if issues.is_empty() {
                ui.label(
                    RichText::new("✔ Ready to convert")
                        .color(egui::Color32::from_rgb(120, 220, 140)),
                );
            } else {
                ui.label(
                    RichText::new(format!("⚠ {}", issues.join(" · ")))
                        .color(egui::Color32::from_rgb(255, 170, 90)),
                );
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let button_text = if self.conversion_in_progress {
                    "Converting…"
                } else {
                    "Convert"
                };
                if gui::primary_button(ui, button_text, can_convert) {
                    self.do_conversion();
                }
            });
        });

        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new("obj2brz · by Smallguy/Kmschr and Suficio")
                    .small()
                    .color(ui.visuals().weak_text_color()),
            );
        });
    }

    fn source_card(&mut self, ui: &mut Ui, input_file_valid: bool) {
        let file_color = gui::bool_color(ui, input_file_valid);
        ui.label(RichText::new("Model file").strong())
            .on_hover_text("3D model to convert (currently OBJ)");
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add(
                TextEdit::singleline(&mut self.input_file_path)
                    .desired_width((ui.available_width() - 48.0).max(120.0))
                    .hint_text("path/to/model.obj")
                    .text_color(file_color),
            );
            #[cfg(not(target_arch = "wasm32"))]
            if gui::file_button(ui) && self.input_file_path_receiver.is_none() {
                let (tx, rx) = mpsc::channel();
                self.input_file_path_receiver = Some(rx);
                thread::spawn(move || {
                    let obj_path = FileDialog::new()
                        .add_filter("3D Model", &["obj"])
                        .pick_file();
                    let _ = tx.send(obj_path);
                });
            }
            #[cfg(target_arch = "wasm32")]
            ui.add_enabled(false, Button::new("🗁"))
                .on_hover_text("File pickers are available in the native application.");
        });

        ui.add_space(8.0);
        self.model_size_estimate(ui);
    }

    fn model_size_estimate(&self, ui: &mut Ui) {
        match self.model_bounds.as_ref() {
            Some(Ok(bounds)) => {
                let [width, depth, height] = bounds.estimated_stud_dimensions(&self.to_options());

                ui.label(RichText::new(format!(
                    "Estimated in-game bounds: ≈ {} studs wide × {} studs deep × {} studs tall",
                    format_studs(width),
                    format_studs(depth),
                    format_studs(height),
                ))
                .strong())
                .on_hover_text(
                    "Based on the OBJ's triangle bounds and your current Scale, Brick Type, and Brick Scale. Voxelization can round the outer edge by up to one voxel.",
                );
            }
            Some(Err(error)) => {
                ui.label(
                    RichText::new(format!("Unable to estimate model size: {error}"))
                        .small()
                        .color(egui::Color32::from_rgb(255, 170, 90)),
                );
            }
            None if self.model_bounds_receiver.is_some() => {
                ui.label(
                    RichText::new("Measuring model bounds…")
                        .small()
                        .color(ui.visuals().weak_text_color()),
                );
            }
            None => {}
        }
    }

    fn output_card(&mut self, ui: &mut Ui, output_dir_valid: bool) {
        let dir_color = gui::bool_color(ui, output_dir_valid);
        ui.label(RichText::new("Output directory").strong())
            .on_hover_text("Where the generated save is written");
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add(
                TextEdit::singleline(&mut self.output_directory)
                    .desired_width((ui.available_width() - 48.0).max(120.0))
                    .text_color(dir_color),
            );
            #[cfg(not(target_arch = "wasm32"))]
            if gui::file_button(ui) && self.output_directory_receiver.is_none() {
                let (tx, rx) = mpsc::channel();
                self.output_directory_receiver = Some(rx);
                let default_dir = self.output_directory.clone();
                thread::spawn(move || {
                    let mut dialog = FileDialog::new();
                    if output_dir_valid {
                        dialog = dialog.set_directory(Path::new(default_dir.as_str()));
                    }
                    let output_dir = dialog.pick_folder();
                    let _ = tx.send(output_dir);
                });
            }
            #[cfg(target_arch = "wasm32")]
            ui.add_enabled(false, Button::new("🗁"))
                .on_hover_text("Browser builds do not have a writable output directory.");
        });

        ui.add_space(10.0);
        gui::form_grid(ui, "output_grid", |ui| {
            ui.label("Save name")
                .on_hover_text("Name for the Brickadia save file");
            ui.add(TextEdit::singleline(&mut self.save_name).desired_width(220.0));
            ui.end_row();

            ui.label("Format").on_hover_text(
                "BRZ is a compact prefab archive. BRDB is an editable Brickadia world directory.",
            );
            ComboBox::from_id_salt("output_format")
                .selected_text(match self.output_format {
                    OutputFormat::Brz => "BRZ (prefab archive)",
                    OutputFormat::Brdb => "BRDB (editable world)",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut self.output_format,
                        OutputFormat::Brz,
                        "BRZ (prefab archive)",
                    );
                    ui.selectable_value(
                        &mut self.output_format,
                        OutputFormat::Brdb,
                        "BRDB (editable world)",
                    );
                });
            ui.end_row();

            #[cfg(target_os = "windows")]
            {
                ui.label("Clipboard");
                ui.checkbox(
                    &mut self.copy_to_clipboard,
                    "Copy save path after generation",
                );
                ui.end_row();
            }
        });
    }

    fn bricks_card(&mut self, ui: &mut Ui) {
        gui::form_grid(ui, "bricks_grid", |ui| {
            ui.label("Brick type")
                .on_hover_text("Which bricks make up the save. Default gives a stud texture.");
            ui.add_enabled_ui(!self.rampify, |ui| {
                ComboBox::from_id_salt("bricktype")
                    .selected_text(format!("{:?}", &mut self.bricktype))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.bricktype, BrickType::Microbricks, "Microbricks");
                        ui.selectable_value(&mut self.bricktype, BrickType::Default, "Default");
                        ui.selectable_value(&mut self.bricktype, BrickType::Tiles, "Tiles");
                    });
            });
            ui.end_row();

            ui.label("Material");
            ComboBox::from_id_salt("material")
                .selected_text(format!("{:?}", &mut self.material))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.material, Material::Plastic, "Plastic");
                    ui.selectable_value(&mut self.material, Material::Glass, "Glass");
                    ui.selectable_value(&mut self.material, Material::Glow, "Glow");
                    ui.selectable_value(&mut self.material, Material::Metallic, "Metallic");
                    ui.selectable_value(&mut self.material, Material::Hologram, "Hologram");
                    ui.selectable_value(&mut self.material, Material::Ghost, "Ghost");
                });
            ui.end_row();

            ui.label("Collision").on_hover_text(
                "Choose which things the exported model should collide with in Brickadia.",
            );
            ui.vertical(|ui| {
                ui.checkbox(&mut self.player_collision, "Players");
                ui.checkbox(&mut self.physics_collision, "Physics / brick grids");
            });
            ui.end_row();

            ui.label("Scale")
                .on_hover_text("Overall size of the generated save");
            ui.add(
                DragValue::new(&mut self.scale)
                    .min_decimals(2)
                    .prefix("x")
                    .speed(0.1),
            );
            ui.end_row();

            ui.label("Simplify").on_hover_text(
                "Merge similar bricks for a less detailed model (reduces brick count)",
            );
            ui.add_enabled(!self.rampify, Checkbox::new(&mut self.simplify, "Lossy — fewer bricks"));
            ui.end_row();

            ui.label("Rampify").on_hover_text(
                "Replace exposed voxels with default ramps and wedges. Runs directly on the voxel octree, without creating one plate brick per voxel.",
            );
            ui.add(Checkbox::new(&mut self.rampify, "Smooth slopes"));
            ui.end_row();
        });
    }

    fn advanced_options(&mut self, ui: &mut Ui, uuid_valid: bool) {
        ui.label("Material Intensity");
        ui.add(Slider::new(
            &mut self.material_intensity,
            std::ops::RangeInclusive::new(0, 10),
        ));
        ui.end_row();

        ui.label("Split by Material (Experimental)")
            .on_hover_text("Process each OBJ material separately into frozen grids");
        ui.add(Checkbox::new(
            &mut self.split_by_material,
            "Separate grids per material",
        ));
        ui.end_row();

        if self.split_by_material {
            ui.label("Grid Offset X")
                .on_hover_text("Horizontal spacing between material grids");
            ui.add(
                DragValue::new(&mut self.grid_offset_x)
                    .suffix(" units")
                    .speed(10.0),
            );
            ui.end_row();

            ui.label("Grid Offset Y")
                .on_hover_text("Forward/back spacing between material grids");
            ui.add(
                DragValue::new(&mut self.grid_offset_y)
                    .suffix(" units")
                    .speed(10.0),
            );
            ui.end_row();

            ui.label("Grid Offset Z")
                .on_hover_text("Vertical spacing between material grids");
            ui.add(
                DragValue::new(&mut self.grid_offset_z)
                    .suffix(" units")
                    .speed(10.0),
            );
            ui.end_row();
        }

        if !self.rampify && self.bricktype == BrickType::Microbricks {
            ui.label("Brick Scale")
                .on_hover_text("Use this to make microbricks bigger for a more pixelated look");
            ui.add(
                DragValue::new(&mut self.brick_scale)
                    .prefix("x")
                    .range(1..=500),
            );
            ui.end_row();
        }

        let id_color = bool_color(ui, uuid_valid);

        ui.label("Brick Owner")
            .on_hover_text("Who will have ownership of the generated bricks");
        ui.horizontal(|ui| {
            ui.add(TextEdit::singleline(&mut self.save_owner_name).desired_width(100.0));
            ui.add(
                TextEdit::singleline(&mut self.save_owner_id)
                    .desired_width(300.0)
                    .text_color(id_color),
            );
        });
        ui.end_row();
    }

    fn show_missing_resources_dialog(&mut self, ctx: &egui::Context) {
        if let Some(message) = &self.missing_resources_dialog.clone() {
            let mut open = true;
            Window::new("⚠ Missing Resources")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .fixed_size([500.0, 400.0])
                .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.vertical(|ui| {
                        ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                            ui.label(message);
                        });

                        ui.add_space(10.);
                        ui.separator();
                        ui.add_space(10.);

                        ui.label("Do you want to continue without the missing textures?");
                        ui.add_space(5.);
                        ui.label("• Yes: Use solid colors from material definitions");
                        ui.label("• No: Cancel conversion so you can fix the missing textures");

                        ui.add_space(10.);
                        ui.horizontal(|ui| {
                            ui.add_space(100.0);
                            if ui.button("Yes").clicked() {
                                self.missing_resources_dialog = None;
                                self.pending_conversion_skip_textures = true;
                                self.continue_conversion(true);
                            }
                            if ui.button("No").clicked() {
                                self.missing_resources_dialog = None;
                                self.pending_conversion_skip_textures = false;
                            }
                        });
                    });
                });

            if !open {
                self.missing_resources_dialog = None;
                self.pending_conversion_skip_textures = false;
            }
        }
    }

    fn do_conversion(&mut self) {
        if let Some(error) = self.to_options().settings_error() {
            MessageDialog::new()
                .set_level(MessageLevel::Error)
                .set_title("Conversion Error")
                .set_description(&error)
                .show();
            return;
        }

        // Validate resources before conversion
        let missing = match validate_obj_resources(&self.input_file_path) {
            Ok(m) => m,
            Err(e) => {
                MessageDialog::new()
                    .set_level(MessageLevel::Error)
                    .set_title("Conversion Error")
                    .set_description(&format!("{}", e))
                    .show();
                return;
            }
        };

        // If there are missing resources, ask user what to do
        if missing.has_issues() {
            let message = format!(
                "The following issues were found:\n\n{}",
                missing.description()
            );
            self.missing_resources_dialog = Some(message);
            return;
        }

        // No missing resources, continue with conversion
        self.continue_conversion(false);
    }

    fn continue_conversion(&mut self, skip_textures: bool) {
        self.conversion_in_progress = true;
        self.logger.log("Starting conversion...".to_string());

        // Create channel to signal completion
        let (tx, rx) = mpsc::channel();
        self.conversion_done_receiver = Some(rx);

        let opts = self.to_options();
        let logger = self.logger.clone();

        // Spawn background thread for conversion
        thread::spawn(move || {
            if let Err(e) = convert(&opts, skip_textures) {
                logger.log(format!("Error: {}", e));
                MessageDialog::new()
                    .set_level(MessageLevel::Error)
                    .set_title("Conversion Failed")
                    .set_description(&format!("{}", e))
                    .show();
            }

            // Signal completion
            let _ = tx.send(());
        });
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    let logger = Logger::new();
    logger.log("obj2brz started - ready to convert OBJ files to Brickadia saves".to_string());

    let build_dir = match env::consts::OS {
        "windows" => dirs::data_local_dir()
            .and_then(|p| p.to_str().map(|s| s.to_string()))
            .map(|s| s + "\\Brickadia\\Saved\\Builds")
            .unwrap_or_else(|| "builds".to_string()),
        // Brickadia runs through Steam Proton on Linux. Prefabs live inside
        // its Windows compatibility prefix rather than under XDG config.
        "linux" => dirs::home_dir()
            .map(|home| {
                home.join(
                    ".steam/steam/steamapps/compatdata/2199420/pfx/drive_c/users/steamuser/AppData/Local/Brickadia/Saved/Prefabs",
                )
                .to_string_lossy()
                .into_owned()
            })
            .unwrap_or_else(|| "prefabs".to_string()),
        _ => "builds".to_string(),
    };

    let build_dir_clone = build_dir.clone();
    let win_option = NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([WINDOW_WIDTH, WINDOW_HEIGHT])
            .with_min_inner_size([960.0, 680.0]),
        ..Default::default()
    };
    let _ = run_native(
        "obj2brz",
        win_option,
        Box::new(move |cc| {
            // Load previous state if available
            let mut app = if let Some(storage) = cc.storage {
                eframe::get_value(storage, eframe::APP_KEY).unwrap_or_else(|| Obj2Brs {
                    output_directory: build_dir_clone.clone(),
                    logger: logger.clone(),
                    ..Default::default()
                })
            } else {
                Obj2Brs {
                    output_directory: build_dir_clone.clone(),
                    logger: logger.clone(),
                    ..Default::default()
                }
            };

            // Migrate the previous Linux default without overwriting a path
            // explicitly selected by the user.
            #[cfg(target_os = "linux")]
            if dirs::config_dir()
                .map(|path| path.join("Epic/Brickadia/Saved/Builds"))
                .map(|path| path.to_string_lossy().into_owned())
                .is_some_and(|legacy_path| app.output_directory == legacy_path)
            {
                app.output_directory = build_dir_clone.clone();
            }

            // Always re-initialize transient fields
            app.logger = logger.clone();
            app.conversion_in_progress = false;
            app.input_file_path_receiver = None;
            app.model_bounds_receiver = None;
            app.model_bounds_path = String::new();
            app.model_bounds = None;
            app.output_directory_receiver = None;
            app.conversion_done_receiver = None;
            app.missing_resources_dialog = None;
            app.pending_conversion_skip_textures = false;

            // Theme + spacing are applied every frame in `update`.
            Ok(Box::new(app))
        }),
    );
}

// Browser entry point. Boots eframe's `WebRunner` against the canvas that the
// host page (see obj2brz-web) supplies, reusing the same [`Obj2Brs`] app the
// native build runs.
#[cfg(target_arch = "wasm32")]
fn main() {
    use eframe::wasm_bindgen::JsCast as _;

    let web_options = eframe::WebOptions::default();

    wasm_bindgen_futures::spawn_local(async {
        let canvas = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_element_by_id("obj2brz_canvas"))
            .expect("page is missing the #obj2brz_canvas element")
            .dyn_into::<web_sys::HtmlCanvasElement>()
            .expect("#obj2brz_canvas is not a <canvas>");

        eframe::WebRunner::new()
            .start(
                canvas,
                web_options,
                Box::new(|cc| {
                    let app = cc
                        .storage
                        .and_then(|storage| eframe::get_value::<Obj2Brs>(storage, eframe::APP_KEY))
                        .unwrap_or_default();
                    Ok(Box::new(app))
                }),
            )
            .await
            .expect("failed to start eframe web runner");
    });
}
