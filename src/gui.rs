use eframe::egui;
use egui::special_emojis::GITHUB;
use egui::{
    Button,
    Color32,
    Context,
    Grid,
    Hyperlink,
    Label,
    RichText,
    Separator,
    TopBottomPanel,
    Ui,
};

const BUTTON_COLOR: Color32 = Color32::from_rgb(15, 98, 254);
const ERROR_COLOR: Color32 = Color32::from_rgb(255, 168, 168);
#[cfg(not(target_arch = "wasm32"))]
const FOLDER_COLOR: Color32 = Color32::from_rgb(255, 206, 70);

pub fn add_grid(ui: &mut Ui, mut contents: impl FnMut(&mut Ui)) {
    Grid::new("")
        .num_columns(2)
        .spacing([40.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            contents(ui)
        });
}

pub fn add_horizontal_line(ui: &mut Ui) {
    ui.add(Separator::default().spacing(20.));
}

pub fn info_text(ui: &mut Ui) {
    ui.horizontal(|ui| {
        ui.label("❓ You can find your own Brickadia ID by visiting");
        ui.add(Hyperlink::from_label_and_url("brickadia.com/account", "https://brickadia.com/account"));
        ui.label("and clicking View Profile");
    });
    ui.label("Your ID will be shown in the URL");
}

pub fn button(ui: &mut Ui, text: &str, enabled: bool) -> bool {
    let text = RichText::new(text).color(Color32::WHITE);
    let b = Button::new(text).fill(BUTTON_COLOR);
    ui.add_enabled(enabled, b).on_hover_text("WARNING! WILL OVERWRITE ANY EXISTING BRS").clicked()
}

#[cfg(not(target_arch = "wasm32"))]
pub fn file_button(ui: &mut Ui) -> bool {
    ui.button(RichText::new("🗁").color(FOLDER_COLOR)).clicked()
}

pub fn bool_color(b: bool) -> Color32 {
    if b {
        Color32::WHITE
    } else {
        ERROR_COLOR
    }
}

pub fn footer(ctx: &Context) {
    TopBottomPanel::bottom("footer").show(ctx, |ui: &mut Ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(10.);
            ui.add(Label::new(RichText::new("obj2brz").monospace()));
            ui.label("by Smallguy/Kmschr and Suficio");
            let text = format!("{} {}", GITHUB, "GitHub");
            ui.add(Hyperlink::from_label_and_url(text, "https://github.com/kmschr/obj2brz"));
            ui.add_space(10.);
        });
    });
}
