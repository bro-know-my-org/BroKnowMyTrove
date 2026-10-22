//! System fonts for user text; a small bundled subset keeps the UI readable offline.
use eframe::egui::{self, FontData, FontDefinitions, FontFamily};
use fontdb::{Database, Family, Query};
use skrifa::MetadataProvider;
use std::{
    collections::{BTreeSet, HashMap, HashSet},
    sync::Arc,
};

const UI_FONT: &[u8] = include_bytes!("../../../assets/fonts/BkmtSansUI.otf");
const SERIF_FONT: &[u8] = include_bytes!("../../../assets/fonts/NotoSerifUI.otf");

pub fn install(ctx: &egui::Context) {
    let mut database = Database::new();
    database.load_system_fonts();
    ctx.set_fonts(definitions(&database));
}

fn definitions(database: &Database) -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    fonts
        .font_data
        .insert("bkmt-ui".into(), Arc::new(FontData::from_static(UI_FONT)));
    fonts.font_data.insert(
        "serif-ui".into(),
        Arc::new(FontData::from_static(SERIF_FONT)),
    );

    let mut registered = HashMap::new();
    let sans = load_system(
        database,
        &mut fonts,
        "system-sans",
        &mut registered,
        true,
        &[
            Family::Name("Segoe UI"),
            Family::Name("Helvetica Neue"),
            Family::SansSerif,
            Family::Name("Noto Sans"),
            Family::Name("DejaVu Sans"),
            Family::Name("Liberation Sans"),
        ],
    );
    let mono = load_system(
        database,
        &mut fonts,
        "system-mono",
        &mut registered,
        true,
        &[Family::Monospace],
    );
    let cjk = load_system(
        database,
        &mut fonts,
        "system-cjk",
        &mut registered,
        false,
        &[
            Family::Name("Microsoft YaHei"),
            Family::Name("PingFang SC"),
            Family::Name("Noto Sans CJK SC"),
            Family::Name("Noto Sans SC"),
            Family::Name("Source Han Sans SC"),
            Family::Name("WenQuanYi Micro Hei"),
            Family::Name("WenQuanYi Zen Hei"),
            Family::Name("DengXian"),
            Family::Name("SimHei"),
            Family::Name("SimSun"),
            Family::Name("Heiti SC"),
            Family::Name("Microsoft JhengHei"),
            Family::Name("PingFang TC"),
            Family::Name("Noto Sans CJK TC"),
            Family::Name("Yu Gothic"),
            Family::Name("Hiragino Sans"),
            Family::Name("Meiryo"),
            Family::Name("Noto Sans CJK JP"),
            Family::Name("Malgun Gothic"),
            Family::Name("Apple SD Gothic Neo"),
            Family::Name("Noto Sans CJK KR"),
            Family::Name("Arial Unicode MS"),
        ],
    );

    // Keep egui's built-in Latin/symbol fallbacks. Only selected system faces are
    // copied into egui, never every installed font. fontdb handles OS directories
    // and TTC face indices, without requiring native FFI in this crate.
    for (family, names) in &mut fonts.families {
        let primary = if *family == FontFamily::Monospace {
            &mono
        } else {
            &sans
        };
        names.splice(0..0, primary.iter().cloned());
        names.extend(cjk.iter().cloned());
        names.push("bkmt-ui".into());
    }
    let mut serif = vec!["serif-ui".into()];
    serif.extend(fonts.families[&FontFamily::Proportional].iter().cloned());
    fonts
        .families
        .insert(FontFamily::Name("serif".into()), serif);
    fonts
}

fn load_system(
    database: &Database,
    fonts: &mut FontDefinitions,
    prefix: &str,
    registered: &mut HashMap<fontdb::ID, String>,
    first_only: bool,
    families: &[Family<'_>],
) -> Vec<String> {
    let mut names = Vec::new();
    let mut covered = BTreeSet::new();
    let mut seen = HashSet::new();
    for family in families {
        let Some(id) = database.query(&Query {
            families: &[*family],
            ..Default::default()
        }) else {
            continue;
        };
        if !seen.insert(id) {
            continue;
        }
        // Reuse aliases/primary faces by ID. Further CJK faces are loaded only
        // when they add glyphs: regional faces in one TTC often cover the same
        // characters, so keeping each full collection would waste memory.
        let select = |bytes: &[u8], index| {
            let face = skrifa::FontRef::from_index(bytes, index).ok()?;
            let characters: BTreeSet<_> = face
                .charmap()
                .mappings()
                .filter(|(_, glyph)| glyph.to_u32() != 0)
                .map(|(ch, _)| ch)
                .collect();
            (first_only || !characters.is_subset(&covered)).then_some(characters)
        };
        let selected = if let Some(name) = registered.get(&id) {
            let font = &fonts.font_data[name];
            select(font.font.as_ref(), font.index).map(|chars| (font.clone(), chars))
        } else {
            database
                .with_face_data(id, |bytes, index| {
                    let chars = select(bytes, index)?;
                    let mut font = FontData::from_owned(bytes.to_vec());
                    font.index = index;
                    Some((Arc::new(font), chars))
                })
                .flatten()
        };
        let Some((font, characters)) = selected else {
            continue;
        };
        covered.extend(characters);
        let name = registered
            .entry(id)
            .or_insert_with(|| {
                let name = format!("{prefix}-{}", names.len());
                fonts.font_data.insert(name.clone(), font);
                name
            })
            .clone();
        names.push(name);
        if first_only {
            break;
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::epaint::text::Fonts;

    fn ui_characters(path: &std::path::Path, characters: &mut std::collections::BTreeSet<char>) {
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                ui_characters(&path, characters);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                characters.extend(
                    std::fs::read_to_string(path)
                        .unwrap()
                        .chars()
                        .filter(|ch| !ch.is_control() && !ch.is_whitespace()),
                );
            }
        }
    }

    #[test]
    fn bundled_fonts_cover_ui_without_system_fonts() {
        let mut fonts = Fonts::new(Default::default(), definitions(&Database::new()));
        let mut characters = Default::default();
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        ui_characters(&root.join("src"), &mut characters);
        ui_characters(&root.join("../vault-i18n/src"), &mut characters);
        for family in [
            FontFamily::Proportional,
            FontFamily::Monospace,
            FontFamily::Name("serif".into()),
        ] {
            let mut font = fonts.fonts.font(&family);
            // Inspect the actual charmap: egui 0.36's has_glyph has false
            // negatives when a fallback also supplies the replacement glyph.
            for ch in &characters {
                assert!(
                    font.characters().contains_key(ch),
                    "{family:?} missing {ch:?}"
                );
            }
            assert!(!font.characters().contains_key(&'\u{9f98}'));
        }
    }

    #[test]
    fn system_collection_supplies_filename_glyphs_and_preserves_face_index() {
        let mut database = Database::new();
        database
            .load_font_file(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../assets/fonts/NotoSansCJK-Regular.ttc"
            ))
            .unwrap();
        let definitions = definitions(&database);
        assert_eq!(definitions.font_data["system-cjk-0"].index, 2);
        assert_eq!(
            definitions
                .font_data
                .keys()
                .filter(|name| name.starts_with("system-cjk"))
                .count(),
            1
        );
        let mut fonts = Fonts::new(Default::default(), definitions);
        assert_filename(&mut fonts);
    }

    #[test]
    fn complementary_system_faces_fill_filename_gaps_without_duplicate_aliases() {
        let mut database = Database::new();
        database.load_font_data(UI_FONT.to_vec());
        let id = database.faces().next().unwrap().id;
        let mut ui = database.face(id).unwrap().clone();
        database.remove_face(id);
        ui.families = vec![
            (
                "Microsoft YaHei".into(),
                fontdb::Language::English_UnitedStates,
            ),
            ("PingFang SC".into(), fontdb::Language::English_UnitedStates),
        ];
        database.push_face_info(ui);

        let mut collection = Database::new();
        collection
            .load_font_file(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../assets/fonts/NotoSansCJK-Regular.ttc"
            ))
            .unwrap();
        let id = collection
            .query(&Query {
                families: &[Family::Name("Noto Sans CJK SC")],
                ..Default::default()
            })
            .unwrap();
        let mut extra = collection.face(id).unwrap().clone();
        extra.families = vec![(
            "Malgun Gothic".into(),
            fontdb::Language::English_UnitedStates,
        )];
        database.push_face_info(extra);

        let definitions = definitions(&database);
        assert_eq!(
            definitions
                .font_data
                .keys()
                .filter(|name| name.starts_with("system-cjk"))
                .count(),
            2
        );
        let mut fonts = Fonts::new(Default::default(), definitions);
        assert_filename(&mut fonts);
    }

    #[test]
    fn unreadable_system_font_keeps_bundled_fallback() {
        let mut database = Database::new();
        database.load_font_data(UI_FONT.to_vec());
        let id = database.faces().next().unwrap().id;
        let mut face = database.face(id).unwrap().clone();
        database.remove_face(id);
        face.families = vec![(
            "Microsoft YaHei".into(),
            fontdb::Language::English_UnitedStates,
        )];
        face.source = fontdb::Source::File("/nonexistent/bkmt-font.ttf".into());
        database.push_face_info(face);
        assert!(
            !definitions(&database)
                .font_data
                .keys()
                .any(|name| name.starts_with("system-cjk"))
        );
        let mut fonts = Fonts::new(Default::default(), definitions(&database));
        for ch in "打开保险库".chars() {
            assert!(
                fonts
                    .fonts
                    .font(&FontFamily::Proportional)
                    .characters()
                    .contains_key(&ch)
            );
        }
    }

    fn assert_filename(fonts: &mut Fonts) {
        for family in [
            FontFamily::Proportional,
            FontFamily::Monospace,
            FontFamily::Name("serif".into()),
        ] {
            for ch in "\u{9f98}.txt".chars() {
                assert!(
                    fonts.fonts.font(&family).characters().contains_key(&ch),
                    "{family:?} missing filename glyph {ch:?}"
                );
            }
            let galley = fonts.with_pixels_per_point(1.0).layout_no_wrap(
                "\u{9f98}.txt".into(),
                egui::FontId::new(14.0, family),
                egui::Color32::WHITE,
            );
            assert!(galley.size().x > 0.0);
        }
    }

    #[test]
    #[ignore = "requires an installed CJK system font"]
    fn installed_system_fonts_render_filename() {
        let mut database = Database::new();
        database.load_system_fonts();
        let definitions = definitions(&database);
        let mut fonts = Fonts::new(Default::default(), definitions);
        assert_filename(&mut fonts);
    }
}
