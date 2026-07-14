use eframe::egui;
use egui::special_emojis::GITHUB;
use egui::{
    Align, Button, Color32, Context, CornerRadius, FontFamily, FontId, Frame, Hyperlink, Layout,
    Margin, RichText, SidePanel, Stroke, TextStyle, TopBottomPanel, Ui, Vec2,
};

/// Brand accent (Brickadia blue).
pub const ACCENT: Color32 = Color32::from_rgb(55, 132, 255);
const ERROR_COLOR: Color32 = Color32::from_rgb(224, 86, 86);
const FOLDER_COLOR: Color32 = Color32::from_rgb(255, 206, 70);

/// Applies the application palette, spacing and typography. Idempotent: safe
/// to call every frame after the user changes themes.
pub fn configure_style(ctx: &Context, dark: bool) {
    let mut style = (*ctx.style()).clone();

    let mut visuals = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    if dark {
        visuals.panel_fill = Color32::from_rgb(30, 41, 56);
        visuals.window_fill = Color32::from_rgb(37, 50, 68);
        visuals.faint_bg_color = Color32::from_rgb(38, 52, 71);
        visuals.extreme_bg_color = Color32::from_rgb(24, 33, 46);
        visuals.text_edit_bg_color = Some(Color32::from_rgb(27, 38, 53));
        visuals.code_bg_color = Color32::from_rgb(31, 43, 59);
        visuals.override_text_color = Some(Color32::from_rgb(241, 245, 249));
        visuals.weak_text_color = Some(Color32::from_rgb(171, 184, 204));
        visuals.window_stroke = Stroke::new(1.0, Color32::from_rgb(69, 86, 109));

        visuals.widgets.noninteractive.bg_fill = visuals.panel_fill;
        visuals.widgets.noninteractive.weak_bg_fill = visuals.faint_bg_color;
        visuals.widgets.noninteractive.bg_stroke =
            Stroke::new(1.0, Color32::from_rgb(69, 86, 109));
        visuals.widgets.noninteractive.fg_stroke =
            Stroke::new(1.0, Color32::from_rgb(241, 245, 249));
        visuals.widgets.inactive.bg_fill = Color32::from_rgb(43, 58, 78);
        visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(43, 58, 78);
        visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(82, 103, 130));
        visuals.widgets.inactive.fg_stroke =
            Stroke::new(1.0, Color32::from_rgb(233, 239, 248));
        visuals.widgets.hovered.bg_fill = Color32::from_rgb(49, 69, 95);
        visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(49, 69, 95);
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(119, 169, 255));
        visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, Color32::WHITE);
        visuals.widgets.active.bg_fill = ACCENT;
        visuals.widgets.active.weak_bg_fill = ACCENT;
        visuals.widgets.active.bg_stroke = Stroke::new(1.0, Color32::from_rgb(158, 196, 255));
        visuals.widgets.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    } else {
        visuals.panel_fill = Color32::from_rgb(244, 247, 251);
        visuals.window_fill = Color32::WHITE;
        visuals.faint_bg_color = Color32::WHITE;
        visuals.extreme_bg_color = Color32::from_rgb(233, 239, 247);
        visuals.text_edit_bg_color = Some(Color32::from_rgb(249, 251, 254));
        visuals.code_bg_color = Color32::from_rgb(238, 243, 249);
        visuals.override_text_color = Some(Color32::from_rgb(24, 34, 52));
        visuals.weak_text_color = Some(Color32::from_rgb(91, 107, 132));
        visuals.window_stroke = Stroke::new(1.0, Color32::from_rgb(205, 216, 230));

        visuals.widgets.noninteractive.bg_fill = visuals.panel_fill;
        visuals.widgets.noninteractive.weak_bg_fill = visuals.faint_bg_color;
        visuals.widgets.noninteractive.bg_stroke =
            Stroke::new(1.0, Color32::from_rgb(205, 216, 230));
        visuals.widgets.noninteractive.fg_stroke =
            Stroke::new(1.0, Color32::from_rgb(24, 34, 52));
        visuals.widgets.inactive.bg_fill = Color32::from_rgb(232, 238, 246);
        visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(239, 244, 250);
        visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(198, 211, 228));
        visuals.widgets.inactive.fg_stroke =
            Stroke::new(1.0, Color32::from_rgb(35, 48, 70));
        visuals.widgets.hovered.bg_fill = Color32::from_rgb(222, 234, 255);
        visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(222, 234, 255);
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT);
        visuals.widgets.hovered.fg_stroke =
            Stroke::new(1.0, Color32::from_rgb(18, 48, 102));
        visuals.widgets.active.bg_fill = ACCENT;
        visuals.widgets.active.weak_bg_fill = ACCENT;
        visuals.widgets.active.bg_stroke = Stroke::new(1.0, Color32::from_rgb(21, 89, 199));
        visuals.widgets.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    }

    visuals.selection.bg_fill = ACCENT.linear_multiply(0.72);
    visuals.selection.stroke = Stroke::new(1.0, ACCENT);
    visuals.hyperlink_color = ACCENT;

    // Consistent rounding across every widget state.
    let radius = CornerRadius::same(6);
    visuals.widgets.noninteractive.corner_radius = radius;
    visuals.widgets.inactive.corner_radius = radius;
    visuals.widgets.hovered.corner_radius = radius;
    visuals.widgets.active.corner_radius = radius;
    visuals.widgets.open.corner_radius = radius;
    style.visuals = visuals;

    style.spacing.item_spacing = Vec2::new(10.0, 11.0);
    style.spacing.button_padding = Vec2::new(12.0, 7.0);
    style.spacing.interact_size.y = 26.0;
    style.spacing.icon_width = 20.0;

    style.text_styles = [
        (
            TextStyle::Heading,
            FontId::new(22.0, FontFamily::Proportional),
        ),
        (TextStyle::Body, FontId::new(15.0, FontFamily::Proportional)),
        (
            TextStyle::Button,
            FontId::new(15.0, FontFamily::Proportional),
        ),
        (
            TextStyle::Monospace,
            FontId::new(13.0, FontFamily::Monospace),
        ),
        (
            TextStyle::Small,
            FontId::new(12.0, FontFamily::Proportional),
        ),
    ]
    .into();

    ctx.set_style(style);
}

/// Top application bar: title, theme toggle and repository link.
pub fn header(ctx: &Context, dark_mode: &mut bool) {
    TopBottomPanel::top("header")
        .exact_height(40.0)
        .frame(
            Frame::default()
                .fill(ctx.style().visuals.panel_fill)
                .stroke(ctx.style().visuals.window_stroke)
                .inner_margin(Margin::symmetric(16, 4)),
        )
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("obj2brz").heading().strong());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let icon = if *dark_mode { "☀" } else { "🌙" };
                    if ui
                        .button(icon)
                        .on_hover_text("Toggle light / dark theme")
                        .clicked()
                    {
                        *dark_mode = !*dark_mode;
                    }
                    ui.add(Hyperlink::from_label_and_url(
                        format!("{} GitHub", GITHUB),
                        "https://github.com/kmschr/obj2brz",
                    ));
                });
            });
        });
}

/// Left navigation with companion Brickadia community tools.
pub fn tools_sidebar(ctx: &Context) {
    SidePanel::left("brickadia_tools")
        .resizable(true)
        .default_width(230.0)
        .width_range(190.0..=320.0)
        .frame(
            Frame::default()
                .fill(ctx.style().visuals.panel_fill)
                .stroke(ctx.style().visuals.window_stroke)
                .inner_margin(Margin::same(14)),
        )
        .show(ctx, |ui| {
            ui.label(
                RichText::new("BRICKADIA TOOLS")
                    .strong()
                    .size(13.0)
                    .color(ui.visuals().weak_text_color()),
            );
            ui.add_space(4.0);
            ui.label(
                RichText::new("More community tools for your builds.")
                    .small()
                    .color(ui.visuals().weak_text_color()),
            );
            ui.add_space(8.0);

            tool_link(
                ui,
                "brs2brz",
                "https://brs2brz.kmschr.com/",
                "Convert legacy Brickadia saves to modern prefabs.",
            );
            tool_link(
                ui,
                "mc2brz",
                "https://mc2brz.kmschr.com/",
                "Convert a Minecraft world to Brickadia.",
            );
            tool_link(
                ui,
                "WireScript",
                "https://wirescript.brickadia.dev/",
                "A compiled language for Brickadia logic gates.",
            );
            tool_link(
                ui,
                "heightmap2brz",
                "https://heightmap.brickadia.dev/",
                "Turn heightmaps and images into Brickadia saves.",
            );
            tool_link(
                ui,
                "Brick Cartographer",
                "https://brickcartographer.kmschr.com/",
                "Create overhead maps of your Brickadia creations.",
            );
            tool_link(
                ui,
                "Brickadia Independent Community",
                "https://forum.brickadia.org/",
                "Unofficial community forums.",
            );
        });
}

fn tool_link(ui: &mut Ui, label: &str, url: &str, description: &str) {
    ui.add(Hyperlink::from_label_and_url(
        RichText::new(label).strong().color(ACCENT),
        url,
    ));
    ui.label(
        RichText::new(description)
            .small()
            .color(ui.visuals().weak_text_color()),
    );
    ui.add_space(8.0);
}

/// A titled, rounded container that groups related settings.
pub fn card(ui: &mut Ui, title: &str, contents: impl FnOnce(&mut Ui)) {
    Frame::default()
        .fill(ui.visuals().faint_bg_color)
        .stroke(Stroke::new(
            1.0,
            ui.visuals().widgets.noninteractive.bg_stroke.color,
        ))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new(title)
                    .strong()
                    .size(13.0)
                    .color(ui.visuals().weak_text_color()),
            );
            ui.add_space(8.0);
            contents(ui);
        });
    ui.add_space(12.0);
}

/// Compact two-column form grid used inside a [`card`].
pub fn form_grid(ui: &mut Ui, id: &str, contents: impl FnOnce(&mut Ui)) {
    egui::Grid::new(id)
        .num_columns(2)
        .spacing([24.0, 10.0])
        .min_col_width(120.0)
        .show(ui, |ui| contents(ui));
}

pub fn info_text(ui: &mut Ui) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.label(RichText::new("Find your Brickadia ID at").small());
        ui.add(Hyperlink::from_label_and_url(
            RichText::new("brickadia.com/account").small(),
            "https://brickadia.com/account",
        ));
        ui.label(RichText::new("Open View Profile; it's shown in the URL.").small());
    });
}

/// Large primary call-to-action button.
pub fn primary_button(ui: &mut Ui, text: &str, enabled: bool) -> bool {
    let label = RichText::new(text)
        .size(16.0)
        .strong()
        .color(Color32::WHITE);
    let button = Button::new(label)
        .fill(ACCENT)
        .corner_radius(CornerRadius::same(8))
        .min_size(Vec2::new(200.0, 42.0));
    ui.add_enabled(enabled, button)
        .on_hover_text("Overwrites any existing save with the same name")
        .clicked()
}

pub fn file_button(ui: &mut Ui) -> bool {
    ui.button(RichText::new("🗁").color(FOLDER_COLOR))
        .on_hover_text("Browse…")
        .clicked()
}

pub fn bool_color(ui: &Ui, b: bool) -> Color32 {
    if b {
        ui.visuals().text_color()
    } else {
        ERROR_COLOR
    }
}

/// Renders a single log line, coloring errors and success distinctly.
pub fn log_line(ui: &mut Ui, message: &str) {
    let color = if message.contains("Error") || message.starts_with('⚠') {
        ERROR_COLOR
    } else if message.contains("Save written") || message.contains("Done") {
        Color32::from_rgb(120, 220, 140)
    } else {
        ui.visuals().weak_text_color()
    };
    ui.label(RichText::new(message).color(color).monospace());
}
