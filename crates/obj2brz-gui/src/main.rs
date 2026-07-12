mod gui;
#[cfg(not(target_arch = "wasm32"))]
mod icon;

use eframe::{egui, egui::*, App};
#[cfg(not(target_arch = "wasm32"))]
use eframe::{run_native, NativeOptions};
use gui::bool_color;
use obj2brz::{
    convert, validate_obj_resources, BrickType, ConvertOptions, Logger, Material, OutputFormat,
};
#[cfg(not(target_arch = "wasm32"))]
use rfd::FileDialog;
use rfd::{MessageDialog, MessageLevel};
use serde::{Deserialize, Serialize};
use std::{
    path::Path, path::PathBuf, sync::mpsc, sync::mpsc::Receiver, thread,
};
#[cfg(not(target_arch = "wasm32"))]
use std::env;
use uuid::Uuid;

#[cfg(not(target_arch = "wasm32"))]
const WINDOW_WIDTH: f32 = 600.;
#[cfg(not(target_arch = "wasm32"))]
const WINDOW_HEIGHT: f32 = 700.;

/// GUI application state. Wraps the UI-agnostic [`ConvertOptions`] with the
/// transient widgets and channels the egui front-end needs.
#[derive(Debug, Serialize, Deserialize)]
pub struct Obj2Brs {
    pub bricktype: BrickType,
    pub brick_scale: isize,
    #[serde(skip)]
    input_file_path_receiver: Option<Receiver<Option<PathBuf>>>,
    input_file_path: String,
    pub match_brickadia_colorset: bool,
    material: Material,
    material_intensity: u32,
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
}

impl Default for Obj2Brs {
    fn default() -> Self {
        Self {
            bricktype: BrickType::Microbricks,
            brick_scale: 1,
            input_file_path_receiver: None,
            input_file_path: "test.obj".into(),
            match_brickadia_colorset: false,
            material: Material::Plastic,
            material_intensity: 5,
            output_directory_receiver: None,
            output_directory: "builds".into(),
            copy_to_clipboard: false,
            output_format: OutputFormat::Brz,
            save_owner_id: "d66c4ad5-59fc-4a9b-80b8-08dedc25bff9".into(),
            save_owner_name: "obj2brz".into(),
            save_name: "test".into(),
            scale: 1.0,
            simplify: false,
            split_by_material: false,
            grid_offset_x: 0.0,
            grid_offset_y: 0.0,
            grid_offset_z: 0.0,
            missing_resources_dialog: None,
            pending_conversion_skip_textures: false,
            logger: Logger::new(),
            conversion_in_progress: false,
            conversion_done_receiver: None,
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
            match_brickadia_colorset: self.match_brickadia_colorset,
            material: self.material,
            material_intensity: self.material_intensity,
            output_directory: self.output_directory.clone(),
            copy_to_clipboard: self.copy_to_clipboard,
            output_format: self.output_format,
            save_owner_id: self.save_owner_id.clone(),
            save_owner_name: self.save_owner_name.clone(),
            save_name: self.save_name.clone(),
            scale: self.scale,
            simplify: self.simplify,
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

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Request repaint to keep updating logs
        ctx.request_repaint();

        self.receive_file_dialog_messages();

        let input_file_valid = Path::new(&self.input_file_path).exists();
        let output_dir_valid = Path::new(&self.output_directory).is_dir();
        let uuid_valid = Uuid::parse_str(&self.save_owner_id).is_ok();
        let options_valid = self.to_options().settings_error().is_none();
        let can_convert = input_file_valid
            && output_dir_valid
            && uuid_valid
            && options_valid
            && !self.conversion_in_progress;

        // Show missing resources dialog if needed
        self.show_missing_resources_dialog(ctx);

        CentralPanel::default().show(ctx, |ui: &mut Ui| {
            ui.vertical(|ui| {
                ScrollArea::vertical()
                    .max_height(400.0)
                    .show(ui, |ui| {
                        gui::add_grid(ui, |ui| self.paths(ui, input_file_valid, output_dir_valid));
                        gui::add_horizontal_line(ui);
                        gui::add_grid(ui, |ui| self.options(ui, uuid_valid));

                        ui.add_space(5.);
                        CollapsingHeader::new("Advanced Options")
                            .default_open(false)
                            .show(ui, |ui| {
                                gui::add_grid(ui, |ui| self.advanced_options(ui, uuid_valid));
                                ui.add_space(5.);
                                gui::info_text(ui);
                            });

                        ui.add_space(10.);
                        ui.horizontal(|ui| {
                            let available_width = ui.available_width();
                            ui.add_space((available_width - 60.0) / 2.0);
                            let button_text = if self.conversion_in_progress {
                                "Converting..."
                            } else {
                                "Voxelize"
                            };
                            if gui::button(ui, button_text, can_convert) {
                                self.do_conversion()
                            }
                        });
                        ui.add_space(10.);
                    });

                // Log panel
                ui.add_space(5.);
                ui.separator();
                ui.add_space(5.);

                Frame::default()
                    .fill(egui::Color32::from_gray(20))
                    .inner_margin(8.0)
                    .show(ui, |ui| {
                        ScrollArea::vertical()
                            .max_height(150.0)
                            .stick_to_bottom(true)
                            .show(ui, |ui| {
                                let messages = self.logger.get_messages();
                                if messages.is_empty() {
                                    ui.label(
                                        RichText::new("No logs yet...")
                                            .color(egui::Color32::GRAY)
                                            .monospace()
                                    );
                                } else {
                                    for message in messages {
                                        ui.label(
                                            RichText::new(message)
                                                .color(egui::Color32::LIGHT_GREEN)
                                                .monospace()
                                        );
                                    }
                                }
                            });
                    });
            });

            gui::footer(ctx);
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

    fn paths(&mut self, ui: &mut Ui, input_file_valid: bool, output_dir_valid: bool) {
        let file_color = gui::bool_color(input_file_valid);

        ui.label("OBJ File").on_hover_text("Model to convert");
        ui.horizontal(|ui| {
            ui.add(
                TextEdit::singleline(&mut self.input_file_path)
                    .desired_width(400.0)
                    .text_color(file_color),
            );
            #[cfg(not(target_arch = "wasm32"))]
            if gui::file_button(ui) && self.input_file_path_receiver.is_none() {
                let (tx, rx) = mpsc::channel();
                self.input_file_path_receiver = Some(rx);
                thread::spawn(move || {
                    let obj_path = FileDialog::new().add_filter("OBJ", &["obj"]).pick_file();
                    let _ = tx.send(obj_path);
                });
            }
            #[cfg(target_arch = "wasm32")]
            ui.add_enabled(false, Button::new("🗁"))
                .on_hover_text("File pickers are available in the native application.");
        });
        ui.end_row();

        let dir_color = gui::bool_color(output_dir_valid);

        ui.label("Output Directory")
            .on_hover_text("Where generated save will be written to");
        ui.horizontal(|ui| {
            ui.add(
                TextEdit::singleline(&mut self.output_directory)
                    .desired_width(400.0)
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
        ui.end_row();

        ui.label("Save Name")
            .on_hover_text("Name for the Brickadia savefile");
        ui.add(TextEdit::singleline(&mut self.save_name));
        ui.end_row();

        ui.label("Output Format")
            .on_hover_text("BRZ is a compact prefab archive. BRDB is an editable Brickadia world directory.");
        ComboBox::from_id_salt("output_format")
            .selected_text(match self.output_format {
                OutputFormat::Brz => "BRZ (prefab archive)",
                OutputFormat::Brdb => "BRDB (editable world)",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.output_format, OutputFormat::Brz, "BRZ (prefab archive)");
                ui.selectable_value(&mut self.output_format, OutputFormat::Brdb, "BRDB (editable world)");
            });
        ui.end_row();

        #[cfg(target_os = "windows")]
        {
            ui.label("");
            ui.checkbox(&mut self.copy_to_clipboard, "Copy to clipboard")
                .on_hover_text("Copy the save file path to clipboard after generation");
            ui.end_row();
        }
    }

    fn options(&mut self, ui: &mut Ui, _uuid_valid: bool) {
        ui.label("Lossy Conversion").on_hover_text(
            "Whether or not to merge similar bricks to create a less detailed model",
        );
        ui.add(Checkbox::new(&mut self.simplify, "Simplify (reduces brickcount)"));
        ui.end_row();

        ui.label("Scale")
            .on_hover_text("Adjusts the overall size of the generated save");
        ui.add(
            DragValue::new(&mut self.scale)
                .min_decimals(2)
                .prefix("x")
                .speed(0.1),
        );
        ui.end_row();

        ui.label("Bricktype")
            .on_hover_text("Which type of bricks will make up the generated save, use default to get a stud texture");
        ComboBox::from_id_salt("bricktype")
            .selected_text(format!("{:?}", &mut self.bricktype))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.bricktype, BrickType::Microbricks, "Microbricks");
                ui.selectable_value(&mut self.bricktype, BrickType::Default, "Default");
                ui.selectable_value(&mut self.bricktype, BrickType::Tiles, "Tiles");
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
    }

    fn advanced_options(&mut self, ui: &mut Ui, uuid_valid: bool) {
        ui.label("Material Intensity");
        ui.add(Slider::new(
            &mut self.material_intensity,
            std::ops::RangeInclusive::new(0, 10),
        ));
        ui.end_row();

        ui.label("Match to Colorset").on_hover_text(
            "Modify the color of the model to match the default color palette in Brickadia",
        );
        ui.add(Checkbox::new(&mut self.match_brickadia_colorset, "Use Default Palette"));
        ui.end_row();

        ui.label("Split by Material (Experimental)").on_hover_text(
            "Process each OBJ material separately into frozen grids",
        );
        ui.add(Checkbox::new(&mut self.split_by_material, "Separate grids per material"));
        ui.end_row();

        if self.split_by_material {
            ui.label("Grid Offset X").on_hover_text(
                "Horizontal spacing between material grids",
            );
            ui.add(DragValue::new(&mut self.grid_offset_x).suffix(" units").speed(10.0));
            ui.end_row();

            ui.label("Grid Offset Y").on_hover_text(
                "Forward/back spacing between material grids",
            );
            ui.add(DragValue::new(&mut self.grid_offset_y).suffix(" units").speed(10.0));
            ui.end_row();

            ui.label("Grid Offset Z").on_hover_text(
                "Vertical spacing between material grids",
            );
            ui.add(DragValue::new(&mut self.grid_offset_z).suffix(" units").speed(10.0));
            ui.end_row();
        }

        if self.bricktype == BrickType::Microbricks {
            ui.label("Brick Scale")
                .on_hover_text("Use this to make microbricks bigger for a more pixelated look");
            ui.add(
                DragValue::new(&mut self.brick_scale)
                    .prefix("x")
                    .range(1..=500),
            );
            ui.end_row();
        }

        let id_color = bool_color(uuid_valid);

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
                        ScrollArea::vertical()
                            .max_height(280.0)
                            .show(ui, |ui| {
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
            let message = format!("The following issues were found:\n\n{}", missing.description());
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
        "windows" => {
            dirs::data_local_dir()
                .and_then(|p| p.to_str().map(|s| s.to_string()))
                .map(|s| s + "\\Brickadia\\Saved\\Builds")
                .unwrap_or_else(|| "builds".to_string())
        }
        "linux" => {
            dirs::config_dir()
                .and_then(|p| p.to_str().map(|s| s.to_string()))
                .map(|s| s + "/Epic/Brickadia/Saved/Builds")
                .unwrap_or_else(|| "builds".to_string())
        }
        _ => "builds".to_string(),
    };

    let build_dir_clone = build_dir.clone();
    let win_option = NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([WINDOW_WIDTH, WINDOW_HEIGHT])
            .with_resizable(false)
            .with_icon(egui::IconData {
                rgba: icon::ICON.to_vec(),
                width: 32,
                height: 32,
            }),
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

            // Always re-initialize transient fields
            app.logger = logger.clone();
            app.conversion_in_progress = false;
            app.input_file_path_receiver = None;
            app.output_directory_receiver = None;
            app.conversion_done_receiver = None;
            app.missing_resources_dialog = None;
            app.pending_conversion_skip_textures = false;

            // Force dark theme
            cc.egui_ctx.set_visuals(egui::Visuals::dark());

            Ok(Box::new(app))
        }),
    );
}

// The browser entry point is supplied by the JavaScript host. Keeping a wasm
// main lets `cargo build --target wasm32-unknown-unknown` produce a valid wasm
// artifact without pulling native windowing symbols into the target.
#[cfg(target_arch = "wasm32")]
fn main() {}
