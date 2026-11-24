//! GitCraft's colours, text sizes and widget look, with the same values GitCraft has
//! always used for its five themes: follow the system, dark, light, and the two high-contrast ones.

use egui::{Color32, CornerRadius, FontFamily, FontId, Stroke, TextStyle, Vec2};
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};

/// A theme choice, stored in the preferences by these ids.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Theme {
    System,
    Dark,
    Light,
    DarkHc,
    LightHc,
}

impl Theme {
    pub const ALL: [Theme; 5] = [Theme::System, Theme::Dark, Theme::Light, Theme::DarkHc, Theme::LightHc];

    pub fn id(self) -> &'static str {
        match self {
            Theme::System => "system",
            Theme::Dark => "dark",
            Theme::Light => "light",
            Theme::DarkHc => "dark-hc",
            Theme::LightHc => "light-hc",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Theme::System => "Follow system",
            Theme::Dark => "Dark",
            Theme::Light => "Light",
            Theme::DarkHc => "Dark (High Contrast)",
            Theme::LightHc => "Light (High Contrast)",
        }
    }

    pub fn from_id(id: &str) -> Theme {
        Theme::ALL.into_iter().find(|t| t.id() == id).unwrap_or(Theme::System)
    }


    /// The theme in use for this choice: never `System`.
    fn resolve(self, ctx: &egui::Context) -> Theme {
        match self {
            Theme::System => match desktop_theme(ctx).or_else(|| ctx.system_theme()) {
                Some(egui::Theme::Light) => Theme::Light,
                _ => Theme::Dark,
            },
            t => t,
        }
    }
}

/// Every colour the interface draws with, for one theme. Names follow the theme tokens GitCraft has always used.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub dark: bool,
    pub app: Color32,
    pub panel: Color32,
    pub panel2: Color32,
    pub toolbar: Color32,
    pub tabs: Color32,
    pub field: Color32,
    pub titlebar: Color32,
    pub menubar: Color32,
    pub line: Color32,
    pub line2: Color32,
    pub link: Color32,
    pub fg: Color32,
    pub fg2: Color32,
    pub fg3: Color32,
    pub selrow: Color32,
    pub hoverrow: Color32,
    pub menu_bg: Color32,
    /// What "white" means in this theme: white on dark themes, near-black on light ones, so
    /// light strokes and fills on dark panels turn into dark ones on light panels.
    ink: Color32,
    /// The opposite of `ink`, for edges and shadows.
    shade: Color32,
}

pub const ACCENT: Color32 = Color32::from_rgb(0x4d, 0x88, 0xff);
pub const GOOD: Color32 = Color32::from_rgb(0x5c, 0xb8, 0x5c);
pub const BAD: Color32 = Color32::from_rgb(0xd9, 0x41, 0x3d);
#[allow(dead_code)] // For the screens still to come, like these two.
pub const WARN: Color32 = Color32::from_rgb(0xde, 0x9b, 0x43);
#[allow(dead_code)]
pub const AI: Color32 = Color32::from_rgb(0x7b, 0x00, 0xff);
/// Text for something removed or stopping, such as Cancel in the status bar.
#[allow(dead_code)]
pub const DELETED_TEXT: Color32 = Color32::from_rgb(0xef, 0x7a, 0x76);

const fn hex(rgb: u32) -> Color32 {
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// `rgb` at `alpha` (0 to 1).
fn alpha(rgb: Color32, a: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(rgb.r(), rgb.g(), rgb.b(), (a * 255.0).round() as u8)
}

impl Palette {
    pub fn of(theme: Theme) -> Palette {
        let white = Color32::WHITE;
        let black = Color32::BLACK;
        let accent_row = |a| alpha(ACCENT, a);
        let dark = Palette {
            dark: true,
            app: hex(0x1c1e23),
            panel: hex(0x272a31),
            panel2: hex(0x2e3138),
            toolbar: hex(0x33373f),
            tabs: hex(0x2a2d34),
            field: hex(0x1f2227),
            titlebar: hex(0x202020),
            menubar: hex(0x1f1f1f),
            line: alpha(white, 0.08),
            line2: alpha(white, 0.16),
            link: hex(0x40c5ec),
            fg: alpha(white, 0.85),
            fg2: alpha(white, 0.6),
            fg3: alpha(white, 0.4),
            selrow: accent_row(0.2),
            hoverrow: accent_row(0.1),
            menu_bg: hex(0x1f1f1f),
            ink: white,
            shade: black,
        };
        let light = Palette {
            dark: false,
            app: hex(0xfcfcfc),
            panel: hex(0xf3f3f3),
            panel2: hex(0xececec),
            toolbar: hex(0xfafafa),
            tabs: hex(0xebebeb),
            field: hex(0xffffff),
            titlebar: hex(0xe8e8e8),
            menubar: hex(0xf0f0f0),
            line: hex(0xc6c6c6),
            line2: hex(0xb0b0b0),
            link: hex(0x005c95),
            fg: alpha(black, 0.75),
            fg2: alpha(black, 0.6),
            fg3: alpha(black, 0.4),
            selrow: Color32::from_rgba_unmultiplied(0x5f, 0x8e, 0xed, 0x33),
            hoverrow: Color32::from_rgba_unmultiplied(0x5f, 0x8e, 0xed, 0x19),
            menu_bg: hex(0xffffff),
            ink: hex(0x111214),
            shade: white,
        };
        match theme {
            Theme::Dark | Theme::System => dark,
            Theme::Light => light,
            Theme::LightHc => Palette {
                fg: Color32::from_rgba_unmultiplied(0, 0, 0, 0xe5),
                fg2: Color32::from_rgba_unmultiplied(0, 0, 0, 0xb3),
                fg3: Color32::from_rgba_unmultiplied(0, 0, 0, 0x99),
                line: hex(0x8a8a8a),
                ..light
            },
            Theme::DarkHc => Palette {
                app: hex(0x090a0b),
                panel: hex(0x141618),
                panel2: hex(0x1d2023),
                toolbar: hex(0x191c20),
                tabs: hex(0x111316),
                field: hex(0x000000),
                titlebar: hex(0x000000),
                menubar: hex(0x050505),
                line: alpha(white, 0.25),
                fg: alpha(white, 0.9),
                fg2: alpha(white, 0.75),
                fg3: alpha(white, 0.55),
                hoverrow: accent_row(0.15),
                menu_bg: hex(0x15171a),
                ..dark
            },
        }
    }

    /// The theme's "white" at `a`: `text-white/80`, `bg-white/10` and the like.
    pub fn white(&self, a: f32) -> Color32 {
        alpha(self.ink, a)
    }

    /// The theme's "black" at `a`: `border-black/30` and the like.
    pub fn black(&self, a: f32) -> Color32 {
        alpha(self.shade, a)
    }
}

static CURRENT: RwLock<Option<Palette>> = RwLock::new(None);
static CHOICE: RwLock<Theme> = RwLock::new(Theme::System);
static FOLLOW: AtomicBool = AtomicBool::new(true);

/// The colours in use.
pub fn p() -> Palette {
    CURRENT.read().ok().and_then(|p| *p).unwrap_or_else(|| Palette::of(Theme::Dark))
}

/// Use `choice` from now on, and look it up again whenever the system changes when it is
/// [`Theme::System`] (see [`follow_system`]).
pub fn choose(choice: Theme, ctx: &egui::Context) {
    FOLLOW.store(choice == Theme::System, Ordering::Relaxed);
    if let Ok(mut c) = CHOICE.write() {
        *c = choice;
    }
    install(Palette::of(choice.resolve(ctx)), ctx);
}

/// Once per frame: when following the system, switch with it.
pub fn follow_system(ctx: &egui::Context) {
    if FOLLOW.load(Ordering::Relaxed) {
        let now = Palette::of(Theme::System.resolve(ctx));
        if now != p() {
            install(now, ctx);
        }
    }
}

fn install(palette: Palette, ctx: &egui::Context) {
    if let Ok(mut c) = CURRENT.write() {
        *c = Some(palette);
    }
    apply_style(ctx);
}

type Watched = Option<(craft_platform::appearance::Watcher, std::sync::Arc<std::sync::OnceLock<egui::Context>>)>;

thread_local! {
    static WATCHED: std::cell::OnceCell<Watched> = const { std::cell::OnceCell::new() };
    /// Whether the first answer was waited for.
    static WAITED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Ask the desktop for its dark or light setting before the window opens, on the thread that
/// draws it, so the answer arrives while the window is created rather than holding up the
/// first frame.
pub fn prestart() {
    WATCHED.with(|w| {
        w.get_or_init(start_watcher);
    });
}

fn start_watcher() -> Watched {
    use std::sync::{Arc, OnceLock};
    let wake = Arc::new(OnceLock::<egui::Context>::new());
    let target = Arc::clone(&wake);
    let watcher = craft_platform::appearance::Watcher::start(move || {
        if let Some(ctx) = target.get() {
            ctx.request_repaint();
        }
    })?;
    Some((watcher, wake))
}

/// The desktop's dark or light setting, watched through the XDG Settings portal so a change shows
/// at once. `None` where there is no portal (macOS and Windows, where the window library tells).
fn desktop_theme(ctx: &egui::Context) -> Option<egui::Theme> {
    use craft_platform::appearance::{Theme as Desktop, sync_native_window_theme};
    WATCHED.with(|w| {
        let (watcher, wake) = w.get_or_init(start_watcher).as_ref()?;
        // So the first frame already has the desktop's look (the answer is usually in by now).
        if !WAITED.replace(true) {
            watcher.wait_first(std::time::Duration::from_millis(250));
        }
        let _ = wake.set(ctx.clone());
        let observed = watcher.current();
        sync_native_window_theme(ctx, observed);
        observed.map(|t| match t {
            Desktop::Dark => egui::Theme::Dark,
            Desktop::Light => egui::Theme::Light,
        })
    })
}

/// Text fields get a border in the light themes, where the field is as white as its panel.
struct FieldBorder;

impl egui::theme::StyleProvider<egui::widget_style::TextEditStyle> for FieldBorder {
    fn style(&mut self, args: &egui::widget_style::StyleArgs<'_>) -> egui::widget_style::TextEditStyle {
        use egui::theme::StyleProvider;
        let mut style: egui::widget_style::TextEditStyle = StyleProvider::style(&mut egui::theme::DefaultStyle, args);
        let frame = &mut style.atom_layout.frame;
        if frame.stroke.width == 0.0 {
            *frame = frame.apply_stroke_and_expansion_without_layout_shift(Stroke::new(1.0, p().white(0.1)), 0.0);
        }
        style
    }
}

pub fn apply_style(ctx: &egui::Context) {
    ctx.add_widget_theme::<egui::widget_style::TextEditStyle>(FieldBorder);
    let c = p();
    let mut style = (*ctx.global_style()).clone();
    style.text_styles = [
        (TextStyle::Heading, FontId::new(15.0, FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(13.0, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(13.0, FontFamily::Proportional)),
        (TextStyle::Small, FontId::new(11.0, FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(12.0, FontFamily::Monospace)),
    ]
    .into();
    let s = &mut style.spacing;
    s.item_spacing = Vec2::new(6.0, 4.0);
    s.button_padding = Vec2::new(7.0, 3.0);
    s.interact_size = Vec2::new(40.0, 22.0);
    s.scroll.bar_width = 7.0;
    s.scroll.floating = true;
    // Menus: 6 px around the items, as craft-menu lays them out by default.
    s.menu_margin = 6.into();
    s.window_margin = 12.into();

    let mut v = if c.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    v.dark_mode = c.dark;
    v.panel_fill = c.app;
    v.window_fill = c.panel;
    v.extreme_bg_color = c.field;
    v.faint_bg_color = c.panel2;
    v.code_bg_color = c.field;
    v.override_text_color = Some(c.fg);
    v.weak_text_color = Some(c.fg2);
    v.hyperlink_color = c.link;
    v.window_stroke = Stroke::new(1.0, c.line);
    v.window_corner_radius = CornerRadius::same(6);
    v.menu_corner_radius = CornerRadius::same(12);
    v.selection.bg_fill = alpha(ACCENT, 0.4);
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.text_cursor.stroke = Stroke::new(1.5, c.fg);
    v.popup_shadow = egui::Shadow { offset: [0, 12], blur: 30, spread: 0, color: Color32::from_black_alpha(102) };
    let round = CornerRadius::same(4);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = c.panel;
    w.noninteractive.bg_stroke = Stroke::new(1.0, c.line);
    w.noninteractive.fg_stroke = Stroke::new(1.0, c.fg);
    w.noninteractive.corner_radius = round;
    w.inactive.bg_fill = c.white(0.1);
    w.inactive.weak_bg_fill = c.white(0.06);
    w.inactive.bg_stroke = Stroke::NONE;
    w.inactive.fg_stroke = Stroke::new(1.0, c.fg);
    w.inactive.corner_radius = round;
    w.hovered.bg_fill = c.white(0.15);
    w.hovered.weak_bg_fill = c.white(0.1);
    w.hovered.bg_stroke = Stroke::NONE;
    w.hovered.fg_stroke = Stroke::new(1.0, c.white(1.0));
    w.hovered.corner_radius = round;
    w.hovered.expansion = 0.0;
    w.active.bg_fill = c.white(0.2);
    w.active.weak_bg_fill = c.white(0.15);
    w.active.bg_stroke = Stroke::NONE;
    w.active.fg_stroke = Stroke::new(1.0, c.white(1.0));
    w.active.corner_radius = round;
    w.active.expansion = 0.0;
    w.open.bg_fill = c.white(0.1);
    w.open.weak_bg_fill = c.white(0.1);
    w.open.bg_stroke = Stroke::NONE;
    w.open.corner_radius = round;
    style.visuals = v;
    // Follow GitCraft's choice rather than egui's guess, so pop-ups use the same colours.
    ctx.options_mut(|o| {
        o.theme_preference = if c.dark { egui::ThemePreference::Dark } else { egui::ThemePreference::Light };
    });
    ctx.set_style_of(egui::Theme::Dark, style.clone());
    ctx.set_style_of(egui::Theme::Light, style);
    // The system's own title bar and dialogs follow the same choice. When following the system,
    // leave them alone, or the window would stop reporting the system's changes.
    let system = if FOLLOW.load(Ordering::Relaxed) {
        egui::SystemTheme::SystemDefault
    } else if c.dark {
        egui::SystemTheme::Dark
    } else {
        egui::SystemTheme::Light
    };
    ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(system));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG contrast ratio of `fg` drawn over `bg`.
    fn contrast(fg: Color32, bg: Color32) -> f32 {
        let [r, g, b, a] = fg.to_srgba_unmultiplied();
        let a = a as f32 / 255.0;
        let over = |c: u8, under: u8| (c as f32 * a + under as f32 * (1.0 - a)) / 255.0;
        let blended = [over(r, bg.r()), over(g, bg.g()), over(b, bg.b())];
        let lum = |c: [f32; 3]| {
            let lin = |v: f32| if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
            0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2])
        };
        let b = [bg.r() as f32 / 255.0, bg.g() as f32 / 255.0, bg.b() as f32 / 255.0];
        let (l1, l2) = (lum(blended), lum(b));
        (l1.max(l2) + 0.05) / (l1.min(l2) + 0.05)
    }

    #[test]
    fn every_theme_keeps_text_readable_on_its_panels() {
        for theme in Theme::ALL {
            let c = Palette::of(theme);
            for bg in [c.app, c.panel, c.toolbar, c.tabs, c.menubar] {
                assert!(contrast(c.fg, bg) >= 7.0, "{theme:?}: body text {:.1}", contrast(c.fg, bg));
                assert!(contrast(c.fg2, bg) >= 4.5, "{theme:?}: secondary text {:.1}", contrast(c.fg2, bg));
            }
        }
    }

    #[test]
    fn theme_ids_round_trip_and_unknown_ids_follow_the_system() {
        for theme in Theme::ALL {
            assert_eq!(Theme::from_id(theme.id()), theme);
        }
        assert_eq!(Theme::from_id("solarized"), Theme::System);
    }
}
