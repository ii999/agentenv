//! Visual design of the credential window: typography, colors and the
//! custom-painted controls. With a system glass backdrop (macOS), surfaces are
//! translucent layers over the material; elsewhere they are opaque.

use std::sync::Arc;

use eframe::egui::{
    self, accesskit::Role, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId,
    Pos2, Rect, Response, Sense, Shadow, Shape, Stroke, StrokeKind, TextStyle, Theme, Ui, Vec2,
    Widget, WidgetInfo,
};

/// Font family used for titles and the primary action.
pub(super) const SEMIBOLD: &str = "semibold";
/// Corner radius of a window whose glass backdrop draws its shape.
pub(super) const WINDOW_CORNER_RADIUS: u8 = 16;
/// Height of every capsule button.
pub(super) const BUTTON_HEIGHT: f32 = 32.0;
/// How far the keyboard focus ring extends outside its control.
pub(super) const FOCUS_RING_OUTSET: f32 = 5.0;
/// Corner radius of the input field and inline banners.
pub(super) const FIELD_RADIUS: CornerRadius = CornerRadius::same(10);

/// Colors for one appearance. Translucent values are composited over the
/// system material, so they stay legible on whatever lies behind the window.
#[derive(Clone, Copy, Debug)]
pub(super) struct Palette {
    pub glass: bool,
    pub text: Color32,
    pub secondary: Color32,
    pub tertiary: Color32,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub accent_pressed: Color32,
    pub on_accent: Color32,
    /// Tint over the backdrop, or the opaque window background.
    pub surface: Color32,
    pub field: Color32,
    pub control: Color32,
    pub control_hover: Color32,
    pub control_pressed: Color32,
    /// Light edge along translucent controls.
    pub rim: Color32,
    pub hairline: Color32,
    pub popup: Color32,
    pub danger: Color32,
    pub warning: Color32,
    pub shadow: Color32,
}

const fn alpha(r: u8, g: u8, b: u8, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied_const(r, g, b, a)
}

impl Palette {
    pub fn new(dark: bool, glass: bool) -> Self {
        match (dark, glass) {
            (false, glass) => Self {
                glass,
                text: Color32::from_rgb(29, 29, 31),
                secondary: alpha(0, 0, 0, 150),
                tertiary: alpha(0, 0, 0, 105),
                accent: Color32::from_rgb(0, 122, 255),
                accent_hover: Color32::from_rgb(28, 138, 255),
                accent_pressed: Color32::from_rgb(0, 100, 216),
                on_accent: Color32::WHITE,
                surface: if glass {
                    alpha(255, 255, 255, 46)
                } else {
                    Color32::from_rgb(242, 242, 247)
                },
                field: if glass {
                    alpha(255, 255, 255, 170)
                } else {
                    Color32::WHITE
                },
                control: if glass {
                    alpha(255, 255, 255, 120)
                } else {
                    Color32::WHITE
                },
                control_hover: if glass {
                    alpha(255, 255, 255, 190)
                } else {
                    Color32::from_rgb(250, 250, 252)
                },
                control_pressed: if glass {
                    alpha(0, 0, 0, 22)
                } else {
                    Color32::from_rgb(229, 229, 234)
                },
                rim: if glass {
                    alpha(255, 255, 255, 220)
                } else {
                    alpha(0, 0, 0, 0)
                },
                hairline: alpha(0, 0, 0, 26),
                popup: alpha(250, 250, 252, 248),
                danger: Color32::from_rgb(215, 0, 21),
                warning: Color32::from_rgb(178, 92, 0),
                shadow: alpha(0, 0, 0, 30),
            },
            (true, glass) => Self {
                glass,
                text: Color32::from_rgb(245, 245, 247),
                secondary: alpha(235, 235, 245, 160),
                tertiary: alpha(235, 235, 245, 110),
                accent: Color32::from_rgb(10, 132, 255),
                accent_hover: Color32::from_rgb(48, 152, 255),
                accent_pressed: Color32::from_rgb(0, 108, 224),
                on_accent: Color32::WHITE,
                surface: if glass {
                    alpha(0, 0, 0, 36)
                } else {
                    Color32::from_rgb(30, 30, 32)
                },
                field: if glass {
                    alpha(0, 0, 0, 72)
                } else {
                    Color32::from_rgb(22, 22, 24)
                },
                control: if glass {
                    alpha(255, 255, 255, 22)
                } else {
                    Color32::from_rgb(50, 50, 53)
                },
                control_hover: if glass {
                    alpha(255, 255, 255, 38)
                } else {
                    Color32::from_rgb(60, 60, 64)
                },
                control_pressed: if glass {
                    alpha(255, 255, 255, 14)
                } else {
                    Color32::from_rgb(42, 42, 45)
                },
                rim: if glass {
                    alpha(255, 255, 255, 40)
                } else {
                    alpha(255, 255, 255, 14)
                },
                hairline: alpha(255, 255, 255, 26),
                popup: alpha(44, 44, 46, 250),
                danger: Color32::from_rgb(255, 69, 58),
                warning: Color32::from_rgb(255, 176, 64),
                shadow: alpha(0, 0, 0, 90),
            },
        }
    }

    pub fn of(ui: &Ui, glass: bool) -> Self {
        Self::new(ui.visuals().dark_mode, glass)
    }
}

/// Installs fonts, text sizes and widget visuals for both appearances.
pub(super) fn install(context: &egui::Context, glass: bool) {
    context.set_fonts(fonts());
    for theme in [Theme::Light, Theme::Dark] {
        context.style_mut_of(theme, |style| {
            style.visuals = visuals(theme == Theme::Dark, glass);
            style.spacing.item_spacing = egui::vec2(8.0, 10.0);
            style.spacing.button_padding = egui::vec2(14.0, 7.0);
            style.spacing.interact_size.y = 28.0;
            style.spacing.menu_margin = egui::Margin::same(6);
            style.text_styles = [
                (TextStyle::Heading, FontId::new(20.0, semibold())),
                (TextStyle::Body, FontId::proportional(13.5)),
                (TextStyle::Button, FontId::proportional(13.5)),
                (TextStyle::Small, FontId::proportional(11.5)),
                (TextStyle::Monospace, FontId::monospace(13.0)),
            ]
            .into();
        });
    }
}

pub(super) fn semibold() -> FontFamily {
    FontFamily::Name(SEMIBOLD.into())
}

/// Places the platform interface font ahead of egui's bundled fonts, which
/// remain as fallbacks. Without a readable platform font the bundled fonts
/// are used alone.
fn fonts() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    let fallback = fonts.families[&FontFamily::Proportional].clone();
    let mut semibold_family = fallback.clone();
    let source = platform_fonts();
    if let Some((regular, semibold)) = source.proportional {
        fonts
            .font_data
            .insert("ui-regular".into(), Arc::new(regular));
        fonts
            .font_data
            .insert("ui-semibold".into(), Arc::new(semibold));
        prepend(&mut fonts, FontFamily::Proportional, "ui-regular");
        semibold_family.insert(0, "ui-semibold".into());
    }
    if let Some(monospace) = source.monospace {
        fonts
            .font_data
            .insert("ui-mono".into(), Arc::new(monospace));
        prepend(&mut fonts, FontFamily::Monospace, "ui-mono");
    }
    fonts.families.insert(semibold(), semibold_family);
    fonts
}

fn prepend(fonts: &mut FontDefinitions, family: FontFamily, name: &str) {
    fonts
        .families
        .entry(family)
        .or_default()
        .insert(0, name.into());
}

#[derive(Default)]
struct PlatformFonts {
    proportional: Option<(FontData, FontData)>,
    monospace: Option<FontData>,
}

#[cfg(target_os = "macos")]
fn platform_fonts() -> PlatformFonts {
    use egui::{epaint::text::VariationCoords, FontTweak};

    let weight = |bytes: &egui::epaint::text::Blob, wght: f32| {
        FontData::from_blob(bytes.clone(), 0).tweak(FontTweak {
            coords: VariationCoords::new([("wght", wght)]),
            ..Default::default()
        })
    };
    let read = |path| {
        std::fs::read(path)
            .ok()
            .map(|bytes| -> egui::epaint::text::Blob { Arc::new(bytes) })
    };
    PlatformFonts {
        // SF Pro is a variable font; one file supplies both weights.
        proportional: read("/System/Library/Fonts/SFNS.ttf")
            .map(|bytes| (weight(&bytes, 400.0), weight(&bytes, 590.0))),
        monospace: read("/System/Library/Fonts/SFNSMono.ttf").map(|bytes| weight(&bytes, 400.0)),
    }
}

#[cfg(windows)]
fn platform_fonts() -> PlatformFonts {
    let directory = std::env::var_os("SystemRoot")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "C:\\Windows".into())
        .join("Fonts");
    let read = |name: &str| {
        std::fs::read(directory.join(name))
            .ok()
            .map(FontData::from_owned)
    };
    PlatformFonts {
        proportional: read("segoeui.ttf").zip(read("seguisb.ttf")),
        monospace: read("consola.ttf"),
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
fn platform_fonts() -> PlatformFonts {
    PlatformFonts::default()
}

fn visuals(dark: bool, glass: bool) -> egui::Visuals {
    let palette = Palette::new(dark, glass);
    let mut visuals = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    let radius = FIELD_RADIUS;
    visuals.panel_fill = palette.surface;
    visuals.window_fill = palette.popup;
    visuals.window_stroke = Stroke::new(1.0, palette.hairline);
    visuals.window_corner_radius = CornerRadius::same(12);
    visuals.menu_corner_radius = CornerRadius::same(10);
    let popup_shadow = Shadow {
        offset: [0, 8],
        blur: 24,
        spread: 0,
        color: palette.shadow,
    };
    visuals.popup_shadow = popup_shadow;
    visuals.window_shadow = popup_shadow;
    visuals.extreme_bg_color = palette.field;
    visuals.text_edit_bg_color = Some(palette.field);
    visuals.faint_bg_color = palette.control;
    visuals.code_bg_color = palette.control;
    visuals.hyperlink_color = palette.accent;
    visuals.error_fg_color = palette.danger;
    visuals.warn_fg_color = palette.warning;
    visuals.selection.bg_fill = palette.accent.gamma_multiply(0.32);
    visuals.selection.stroke = Stroke::new(1.5, palette.accent);
    visuals.text_cursor.stroke = Stroke::new(2.0, palette.accent);
    visuals.weak_text_color = Some(palette.tertiary);
    visuals.collapsing_header_frame = false;
    visuals.disabled_alpha = 0.45;
    visuals.widgets.noninteractive = egui::style::WidgetVisuals {
        bg_fill: Color32::TRANSPARENT,
        weak_bg_fill: Color32::TRANSPARENT,
        bg_stroke: Stroke::new(1.0, palette.hairline),
        corner_radius: radius,
        fg_stroke: Stroke::new(1.0, palette.text),
        expansion: 0.0,
    };
    visuals.widgets.inactive = egui::style::WidgetVisuals {
        bg_fill: palette.control,
        weak_bg_fill: palette.control,
        bg_stroke: Stroke::new(1.0, palette.hairline),
        corner_radius: radius,
        fg_stroke: Stroke::new(1.0, palette.text),
        expansion: 0.0,
    };
    visuals.widgets.hovered = egui::style::WidgetVisuals {
        bg_fill: palette.control_hover,
        weak_bg_fill: palette.control_hover,
        bg_stroke: Stroke::new(1.0, palette.hairline.gamma_multiply(1.6)),
        ..visuals.widgets.inactive
    };
    visuals.widgets.active = egui::style::WidgetVisuals {
        bg_fill: palette.control_pressed,
        weak_bg_fill: palette.control_pressed,
        bg_stroke: Stroke::new(1.0, palette.accent),
        ..visuals.widgets.inactive
    };
    visuals.widgets.open = visuals.widgets.hovered;
    visuals
}

/// A capsule button drawn as a glass control. The primary kind carries the
/// accent color.
pub(super) struct GlassButton<'a> {
    label: &'a str,
    kind: Kind,
    glass: bool,
    min_width: f32,
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Primary,
    Secondary,
}

impl<'a> GlassButton<'a> {
    pub fn primary(label: &'a str, glass: bool) -> Self {
        Self {
            label,
            kind: Kind::Primary,
            glass,
            min_width: 0.0,
        }
    }
    pub fn secondary(label: &'a str, glass: bool) -> Self {
        Self {
            kind: Kind::Secondary,
            ..Self::primary(label, glass)
        }
    }
    pub fn min_width(mut self, width: f32) -> Self {
        self.min_width = width;
        self
    }
}

impl Widget for GlassButton<'_> {
    fn ui(self, ui: &mut Ui) -> Response {
        let palette = Palette::of(ui, self.glass);
        let primary = self.kind == Kind::Primary;
        let family = if primary {
            semibold()
        } else {
            FontFamily::Proportional
        };
        let galley = ui.painter().layout_no_wrap(
            self.label.to_owned(),
            FontId::new(13.5, family),
            Color32::PLACEHOLDER,
        );
        let height = BUTTON_HEIGHT;
        let size = egui::vec2(
            (galley.size().x + 2.0 * if primary { 18.0 } else { 14.0 }).max(self.min_width),
            height,
        );
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        let enabled = ui.is_enabled();
        response.widget_info(|| WidgetInfo::labeled(Role::Button, enabled, self.label));
        if !ui.is_rect_visible(rect) {
            return response;
        }
        let pressed = response.is_pointer_button_down_on();
        let hovered = response.hovered();
        let (fill, text) = match (primary, pressed, hovered) {
            (true, true, _) => (palette.accent_pressed, palette.on_accent),
            (true, false, true) => (palette.accent_hover, palette.on_accent),
            (true, false, false) => (palette.accent, palette.on_accent),
            (false, true, _) => (palette.control_pressed, palette.text),
            (false, false, true) => (palette.control_hover, palette.text),
            (false, false, false) => (palette.control, palette.text),
        };
        let radius = CornerRadius::same((height / 2.0) as u8);
        let painter = ui.painter();
        if enabled && (primary || palette.glass) {
            let color = if primary {
                palette
                    .accent
                    .gamma_multiply(if palette.glass { 0.35 } else { 0.25 })
            } else {
                palette.shadow.gamma_multiply(0.5)
            };
            painter.add(
                Shadow {
                    offset: [0, 2],
                    blur: if primary { 10 } else { 6 },
                    spread: 0,
                    color,
                }
                .as_shape(rect, radius),
            );
        }
        painter.rect_filled(rect, radius, fill);
        let edge = if primary {
            Color32::from_white_alpha(56)
        } else {
            palette.rim
        };
        painter.rect_stroke(rect, radius, Stroke::new(1.0, edge), StrokeKind::Inside);
        if !primary {
            painter.rect_stroke(
                rect,
                radius,
                Stroke::new(1.0, palette.hairline),
                StrokeKind::Outside,
            );
        }
        if response.has_focus() {
            focus_ring(painter, rect, radius, palette);
        }
        let position = rect.center() - galley.size() / 2.0;
        painter.galley(position, galley, text);
        response
    }
}

/// Side of the square reveal toggle inside the input field.
pub(super) const REVEAL_TOGGLE_SIZE: f32 = 28.0;

/// The eye button that shows or hides the value. It draws an open eye while
/// the value is masked and a slashed eye while it is visible, and reports a
/// checked state to accessibility.
pub(super) struct RevealToggle {
    revealed: bool,
    glass: bool,
}

impl RevealToggle {
    pub fn new(revealed: bool, glass: bool) -> Self {
        Self { revealed, glass }
    }
}

impl Widget for RevealToggle {
    fn ui(self, ui: &mut Ui) -> Response {
        let palette = Palette::of(ui, self.glass);
        let (rect, response) =
            ui.allocate_exact_size(Vec2::splat(REVEAL_TOGGLE_SIZE), Sense::click());
        let enabled = ui.is_enabled();
        response.widget_info(|| {
            WidgetInfo::selected(Role::CheckBox, enabled, self.revealed, "Show value")
        });
        if !ui.is_rect_visible(rect) {
            return response;
        }
        let painter = ui.painter();
        let radius = CornerRadius::same(8);
        let hovered = response.hovered();
        if response.is_pointer_button_down_on() {
            painter.rect_filled(rect, radius, palette.control_pressed);
        } else if hovered {
            painter.rect_filled(rect, radius, palette.control_hover);
        }
        if response.has_focus() {
            focus_ring(painter, rect, radius, palette);
        }
        let color = if hovered {
            palette.text
        } else {
            palette.secondary
        };
        let stroke = Stroke::new(1.5, color);
        let center = rect.center();
        let (half_width, lid) = (8.0, 6.5);
        let left = center - egui::vec2(half_width, 0.0);
        let right = center + egui::vec2(half_width, 0.0);
        for direction in [-1.0, 1.0] {
            let control = |x: f32| center + egui::vec2(x, direction * lid);
            painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                [left, control(-4.0), control(4.0), right],
                false,
                Color32::TRANSPARENT,
                stroke,
            ));
        }
        painter.circle_stroke(center, 2.6, stroke);
        if self.revealed {
            let reach = egui::vec2(7.0, 7.0);
            painter.line_segment([center - reach, center + reach], stroke);
        }
        response.on_hover_text(if self.revealed {
            "Hide value"
        } else {
            "Show value"
        })
    }
}

pub(super) fn focus_ring(
    painter: &egui::Painter,
    rect: Rect,
    radius: CornerRadius,
    palette: Palette,
) {
    // A 2-point gap plus a 3-point ring: FOCUS_RING_OUTSET in total.
    painter.rect_stroke(
        rect.expand(2.0),
        radius + 2,
        Stroke::new(3.0, palette.accent.gamma_multiply(0.45)),
        StrokeKind::Outside,
    );
}

/// The window's emblem: a padlock on an accent-colored rounded square.
pub(super) fn emblem(ui: &mut Ui, palette: Palette) -> Response {
    let size = Vec2::splat(44.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter();
    let radius = CornerRadius::same(12);
    painter.add(
        Shadow {
            offset: [0, 4],
            blur: 14,
            spread: 0,
            color: palette.accent.gamma_multiply(0.4),
        }
        .as_shape(rect, radius),
    );
    painter.rect_filled(rect, radius, palette.accent);
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, Color32::from_white_alpha(70)),
        StrokeKind::Inside,
    );
    let white = palette.on_accent;
    let center = rect.center() + egui::vec2(0.0, 3.0);
    let body = Rect::from_center_size(center, egui::vec2(18.0, 13.0));
    let shackle_radius = 5.5;
    let shackle_base = body.top() + 0.5;
    let shackle_center = Pos2::new(center.x, shackle_base - 4.0);
    let mut points = vec![Pos2::new(center.x - shackle_radius, shackle_base)];
    points.extend((0..=16).map(|step| {
        let angle = std::f32::consts::PI * (1.0 + step as f32 / 16.0);
        shackle_center + shackle_radius * egui::vec2(angle.cos(), angle.sin())
    }));
    points.push(Pos2::new(center.x + shackle_radius, shackle_base));
    painter.add(Shape::line(points, Stroke::new(2.2, white)));
    painter.rect_filled(body, CornerRadius::same(3), white);
    painter.circle_filled(body.center() - egui::vec2(0.0, 1.0), 1.8, palette.accent);
    painter.line_segment(
        [
            body.center() - egui::vec2(0.0, 0.5),
            body.center() + egui::vec2(0.0, 3.0),
        ],
        Stroke::new(1.6, palette.accent),
    );
    response
}
