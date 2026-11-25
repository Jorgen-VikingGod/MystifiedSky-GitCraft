//! The interface font, Open Sans (SIL Open Font License 1.1, see `assets/fonts/LICENSE-OpenSans`),
//! in its regular, semibold and bold weights. DejaVu Sans fills in the symbols and scripts Open
//! Sans lacks (arrows, check marks, other alphabets in file names), and DejaVu Sans Mono draws
//! code (see `assets/fonts/LICENSE-DejaVu`).

use egui::epaint::text::{FontInsert, FontPriority, InsertFontFamily};
use egui::{Context, FontData, FontDefinitions, FontFamily, FontId};
use std::sync::{Arc, OnceLock};
use unicode_properties::emoji::{EmojiStatus, UnicodeEmoji};
#[cfg(windows)]
use unicode_segmentation::UnicodeSegmentation;

const SEMIBOLD: &str = "semibold";
const BOLD: &str = "bold";
const EMOJI_FACES: [&str; 2] = ["NotoEmoji-Regular", "emoji-icon-font"];
#[cfg(windows)]
const WINDOWS_EMOJI: &str = "GitCraft Windows Emoji";
#[cfg(windows)]
const EMOJI_FAMILY: &str = "editor-emoji";
const MONO_BYTES: &[u8] = include_bytes!("../../assets/fonts/DejaVuSansMono.ttf");

pub fn install(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let fallbacks = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    for (name, bytes) in [
        (
            "OpenSans-Regular",
            &include_bytes!("../../assets/fonts/OpenSans-Regular.ttf")[..],
        ),
        (
            "OpenSans-SemiBold",
            &include_bytes!("../../assets/fonts/OpenSans-SemiBold.ttf")[..],
        ),
        (
            "OpenSans-Bold",
            &include_bytes!("../../assets/fonts/OpenSans-Bold.ttf")[..],
        ),
        (
            "DejaVuSans",
            &include_bytes!("../../assets/fonts/DejaVuSans.ttf")[..],
        ),
        ("DejaVuSansMono", MONO_BYTES),
    ] {
        fonts
            .font_data
            .insert(name.to_owned(), Arc::new(FontData::from_static(bytes)));
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
    #[cfg(windows)]
    fonts.families.insert(
        FontFamily::Name(EMOJI_FAMILY.into()),
        EMOJI_FACES.into_iter().map(str::to_owned).collect(),
    );
    #[cfg(windows)]
    let families = fonts.families.keys().cloned().collect();
    ctx.set_fonts(fonts);
    #[cfg(windows)]
    load_windows_emoji(ctx, families);
}

#[cfg(windows)]
fn windows_emoji_face() -> Option<FontData> {
    static WINDOWS_FACE: OnceLock<Option<FontData>> = OnceLock::new();
    WINDOWS_FACE
        .get_or_init(|| {
            let root = std::env::var_os("SystemRoot")?;
            let path = std::path::PathBuf::from(root)
                .join("Fonts")
                .join("seguiemj.ttf");
            let bytes = std::fs::read(path).ok()?;
            Some(FontData::from_blob(Arc::new(bytes), 0))
        })
        .clone()
}

#[cfg(windows)]
fn load_windows_emoji(ctx: &Context, families: Vec<FontFamily>) {
    // The system font stays on the machine. Use the bundled faces if it is unavailable.
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let Some(data) = windows_emoji_face() else {
            return;
        };
        ctx.add_font(FontInsert::new(
            WINDOWS_EMOJI,
            data,
            families
                .into_iter()
                .map(|family| {
                    let priority = if family == FontFamily::Name(EMOJI_FAMILY.into()) {
                        FontPriority::Highest
                    } else {
                        FontPriority::Lowest
                    };
                    InsertFontFamily { family, priority }
                })
                .collect(),
        ));
        ctx.request_repaint();
    });
}

pub(super) fn emoji_cluster(cluster: &str) -> bool {
    if let Some(tags) = cluster
        .strip_prefix('\u{1f3f4}')
        .and_then(|s| s.strip_suffix('\u{e007f}'))
        && !tags.is_empty()
        && tags.chars().all(|c| matches!(c, '\u{e0061}'..='\u{e007a}'))
    {
        return true;
    }
    if let Some(key) = cluster.strip_suffix('\u{20e3}') {
        let key = key.strip_suffix('\u{fe0f}').unwrap_or(key);
        return key.len() == 1
            && key
                .bytes()
                .all(|c| c.is_ascii_digit() || matches!(c, b'#' | b'*'));
    }
    let mut flag = cluster.chars();
    let regional = |c: char| matches!(c, '\u{1f1e6}'..='\u{1f1ff}');
    if flag.next().is_some_and(regional)
        && flag.next().is_some_and(regional)
        && flag.next().is_none()
    {
        return true;
    }
    let joined = cluster.contains('\u{200d}');
    cluster.split('\u{200d}').all(|part| {
        let mut chars = part.chars().peekable();
        let Some(base) = chars.next().filter(|c| !c.is_ascii() && c.is_emoji_char()) else {
            return false;
        };
        // U+FE0E asks for the text style, so such a sequence doesn't belong to the emoji font.
        if chars.next_if_eq(&'\u{fe0e}').is_some() {
            return false;
        }
        let presentation = chars.next_if_eq(&'\u{fe0f}').is_some();
        let modifier = chars
            .next_if(|c| matches!(c, '\u{1f3fb}'..='\u{1f3ff}'))
            .is_some();
        let status = base.emoji_status();
        let valid_modifier = matches!(
            status,
            EmojiStatus::EmojiModifierBase | EmojiStatus::EmojiPresentationAndModifierBase
        );
        let default_emoji = matches!(
            status,
            EmojiStatus::EmojiPresentation
                | EmojiStatus::EmojiPresentationAndModifierBase
                | EmojiStatus::EmojiPresentationAndEmojiComponent
                | EmojiStatus::EmojiPresentationAndModifierAndEmojiComponent
        );
        chars.next().is_none()
            && (!modifier || valid_modifier)
            && (joined || presentation || modifier || default_emoji)
    })
}

pub(super) fn use_emoji_font(ctx: &Context) -> bool {
    #[cfg(windows)]
    {
        ctx.fonts(|fonts| fonts.definitions().font_data.contains_key(WINDOWS_EMOJI))
    }
    #[cfg(not(windows))]
    {
        let _ = ctx;
        false
    }
}

// Choose one face for the whole grapheme. The editor's normal fallback starts with the first
// scalar, which can be in an older text font that has no ligature for the complete emoji.
pub(super) fn compose_emoji(job: &mut egui::text::LayoutJob, enabled: bool) {
    #[cfg(windows)]
    {
        use egui::epaint::text::ByteIndex;
        if !enabled || job.text.is_ascii() {
            return;
        }
        let mut ranges = job
            .text
            .grapheme_indices(true)
            .filter(|(_, cluster)| !cluster.is_ascii() && emoji_cluster(cluster))
            .map(|(start, cluster)| start..start + cluster.len())
            .peekable();
        if ranges.peek().is_none() {
            return;
        }
        let text_sections = std::mem::take(&mut job.sections);
        for section in text_sections {
            let end = section.byte_range.end.0;
            let mut start = section.byte_range.start.0;
            let mut leading = section.leading_space;
            while start < end {
                while ranges.peek().is_some_and(|emoji| emoji.end <= start) {
                    ranges.next();
                }
                let (next, emoji) = match ranges.peek() {
                    Some(range) if range.start <= start => (end.min(range.end), true),
                    Some(range) => (end.min(range.start), false),
                    None => (end, false),
                };
                let mut format = section.format.clone();
                if emoji {
                    format.font_id.family = FontFamily::Name(EMOJI_FAMILY.into());
                }
                job.sections.push(egui::text::LayoutSection {
                    leading_space: leading,
                    byte_range: ByteIndex(start)..ByteIndex(next),
                    format,
                });
                leading = 0.0;
                start = next;
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (job, enabled);
    }
}

/// Resolve an explicit font preference without scanning the system's fonts on the UI thread.
/// The bundled mono font covers initial frames and machines without the selected family.
pub fn preferred(ctx: &Context, name: &str, size: f32) -> FontId {
    if name.is_empty() {
        return FontId::monospace(size);
    }
    let key = format!("system:{name}");
    let family = FontFamily::Name(key.clone().into());
    if ctx.fonts(|fonts| fonts.definitions().families.contains_key(&family)) {
        return FontId::new(size, family);
    }

    let request = egui::Id::NULL.with(("system_font", name));
    let started = ctx.data_mut(|data| {
        let started = data.get_temp::<bool>(request).unwrap_or_default();
        if !started {
            data.insert_temp(request, true);
        }
        started
    });
    if !started {
        let ctx = ctx.clone();
        let name = name.to_owned();
        std::thread::spawn(move || {
            static SYSTEM: OnceLock<fontdb::Database> = OnceLock::new();
            let db = SYSTEM.get_or_init(|| {
                let mut db = fontdb::Database::new();
                db.load_system_fonts();
                db
            });
            if let Some(data) = regular_face(db, &name) {
                ctx.add_font(FontInsert::new(
                    &key,
                    data,
                    vec![InsertFontFamily {
                        family: family.clone(),
                        priority: FontPriority::Highest,
                    }],
                ));
                ctx.add_font(FontInsert::new(
                    &format!("{key}:fallback"),
                    FontData::from_static(MONO_BYTES),
                    vec![InsertFontFamily {
                        family: family.clone(),
                        priority: FontPriority::Lowest,
                    }],
                ));
                let emoji = ctx.fonts(|fonts| {
                    EMOJI_FACES
                        .iter()
                        .filter_map(|face| {
                            fonts
                                .definitions()
                                .font_data
                                .get(*face)
                                .map(|data| (*face, data.clone()))
                        })
                        .collect::<Vec<_>>()
                });
                for (face, data) in emoji {
                    ctx.add_font(FontInsert::new(
                        &format!("{key}:{face}"),
                        (*data).clone(),
                        vec![InsertFontFamily {
                            family: family.clone(),
                            priority: FontPriority::Lowest,
                        }],
                    ));
                }
                #[cfg(windows)]
                if let Some(data) = windows_emoji_face() {
                    ctx.add_font(FontInsert::new(
                        &format!("{key}:{WINDOWS_EMOJI}"),
                        data,
                        vec![InsertFontFamily {
                            family,
                            priority: FontPriority::Lowest,
                        }],
                    ));
                }
                ctx.request_repaint();
            }
        });
    }
    FontId::monospace(size)
}

fn regular_face(db: &fontdb::Database, name: &str) -> Option<FontData> {
    let face = db.query(&fontdb::Query {
        families: &[fontdb::Family::Name(name)],
        ..Default::default()
    })?;
    db.with_face_data(face, |bytes, index| {
        FontData::from_blob(Arc::new(bytes.to_vec()), index)
    })
}

#[cfg(test)]
#[test]
fn headless_font_preference_uses_the_requested_family() {
    let mut db = fontdb::Database::new();
    db.load_font_data(MONO_BYTES.to_vec());
    let face = regular_face(&db, "DejaVu Sans Mono").expect("Bundled font by family name");
    assert_eq!(face.bytes(), MONO_BYTES);
    assert!(regular_face(&db, "Unavailable Family").is_none());
}

#[cfg(test)]
#[test]
fn headless_editor_fonts_cover_common_emoji() {
    let ctx = Context::default();
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
        install(ui.ctx());
        let font = FontId::monospace(12.0);
        assert!(ui.fonts_mut(|fonts| fonts.has_glyphs(&font, "😀👩💻🍕🌈")));
    });
    output.textures_delta.clear();
}

#[cfg(all(test, windows))]
#[test]
fn headless_emoji_font_keeps_complete_clusters_and_leaves_text_alone() {
    let text = "café 1️⃣ 👩🏽‍💻";
    let mut layout = egui::text::LayoutJob::simple_singleline(
        text.to_owned(),
        FontId::monospace(12.0),
        egui::Color32::WHITE,
    );
    compose_emoji(&mut layout, true);
    assert_eq!(layout.text, text);
    let emoji: Vec<_> = layout
        .sections
        .iter()
        .filter(|section| section.format.font_id.family == FontFamily::Name(EMOJI_FAMILY.into()))
        .map(|section| &layout.text[section.byte_range.start.0..section.byte_range.end.0])
        .collect();
    assert_eq!(emoji, ["1️⃣", "👩🏽‍💻"]);
}

pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(SEMIBOLD.into()))
}

pub fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(BOLD.into()))
}
