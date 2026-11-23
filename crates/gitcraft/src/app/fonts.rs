//! The interface font, Open Sans (SIL Open Font License 1.1, see `assets/fonts/LICENSE-OpenSans`),
//! in its regular, semibold and bold weights. DejaVu Sans fills in the symbols and scripts Open
//! Sans lacks (arrows, check marks, other alphabets in file names), and DejaVu Sans Mono draws
//! code (see `assets/fonts/LICENSE-DejaVu`).

use egui::{FontData, FontDefinitions, FontFamily, FontId};
use std::sync::Arc;

const SEMIBOLD: &str = "semibold";
const BOLD: &str = "bold";

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let fallbacks = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    for (name, bytes) in [
        ("OpenSans-Regular", &include_bytes!("../../assets/fonts/OpenSans-Regular.ttf")[..]),
        ("OpenSans-SemiBold", &include_bytes!("../../assets/fonts/OpenSans-SemiBold.ttf")[..]),
        ("OpenSans-Bold", &include_bytes!("../../assets/fonts/OpenSans-Bold.ttf")[..]),
        ("DejaVuSans", &include_bytes!("../../assets/fonts/DejaVuSans.ttf")[..]),
        ("DejaVuSansMono", &include_bytes!("../../assets/fonts/DejaVuSansMono.ttf")[..]),
    ] {
        fonts.font_data.insert(name.to_owned(), Arc::new(FontData::from_static(bytes)));
    }
    let proportional = fonts.families.entry(FontFamily::Proportional).or_default();
    proportional.insert(0, "OpenSans-Regular".to_owned());
    proportional.insert(1, "DejaVuSans".to_owned());
    let mono = fonts.families.entry(FontFamily::Monospace).or_default();
    mono.insert(0, "DejaVuSansMono".to_owned());
    mono.push("DejaVuSans".to_owned());
    for (family, font) in [(SEMIBOLD, "OpenSans-SemiBold"), (BOLD, "OpenSans-Bold")] {
        let mut list = vec![font.to_owned(), "DejaVuSans".to_owned()];
        list.extend(fallbacks.iter().cloned());
        fonts.families.insert(FontFamily::Name(family.into()), list);
    }
    ctx.set_fonts(fonts);
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(SEMIBOLD.into()))
}

pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(BOLD.into()))
}
