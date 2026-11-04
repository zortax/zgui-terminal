//! Which face a cell is drawn from, against the faces this machine has.
//!
//! The answers depend on the installed fonts, so each test first asks whether the faces it is
//! about exist and passes without a claim when they do not.

use zgui::app::{Fonts, Shaper};
use zgui::text::FontSource;
use zgui_terminal::CellMetrics;
use zgui_terminal::TerminalConfig;
use zgui_terminal::render::families;
use zgui_terminal::render::presentation;
use zgui_terminal::render::shaping::{FaceSettings, RunFace, ShapingCache};

/// The faces leash asks for, which is a common shape for a configuration.
const WANTED: [&str; 3] = ["Geist Mono", "DejaVu Sans Mono", "monospace"];

/// The size the cells are shaped at, in device pixels.
const SIZE: f32 = 13.0;

/// A shaper over the system faces, and the cache a grid shapes through.
fn setup() -> (Fonts, Shaper, ShapingCache, CellMetrics) {
    let fonts = Fonts::system();
    let shaper = fonts.shaper();
    let wanted: Vec<String> = WANTED.iter().map(|name| (*name).to_owned()).collect();
    let resolved = families::resolve(&shaper, &wanted, SIZE);
    let cache = ShapingCache::new(resolved, FaceSettings::default());
    let metrics = CellMetrics {
        font_size: SIZE as u32,
        ..CellMetrics::default()
    };
    (fonts, shaper, cache, metrics)
}

/// Whether this machine has a face named `family`.
fn installed(shaper: &Shaper, family: &str) -> bool {
    let family = [zgui_interned::Ident::new(family)];
    shaper
        .line_metrics(&families::request(&family, SIZE))
        .is_some()
}

/// What `text` is drawn from: per glyph, whether it is a real glyph, and whether its face is a
/// colour face.
fn drawn(text: &str) -> Vec<(bool, bool, String)> {
    let (fonts, mut shaper, mut cache, metrics) = setup();
    let face = RunFace {
        emoji: presentation::is_emoji(text),
        ..RunFace::default()
    };
    let runs = cache.shape(&mut shaper, text, face, &metrics);
    let source = shaper.fonts();
    let _ = &fonts;
    runs.iter()
        .flat_map(|run| {
            let family = source
                .face(run.face)
                .map(|record| record.family.as_str().to_owned())
                .unwrap_or_default();
            run.glyphs
                .iter()
                .map(move |glyph| (glyph.glyph != 0, run.has_color, family.clone()))
                .collect::<Vec<_>>()
        })
        .collect()
}

#[test]
fn the_symbols_of_a_mode_line_draw_as_text_glyphs() {
    let (_, shaper, _, _) = setup();
    if !installed(&shaper, "Noto Sans Symbols 2") && !installed(&shaper, "Adwaita Mono") {
        eprintln!("no symbol face is installed");
        return;
    }
    for symbol in ["⏵", "⏸", "⏴", "⏶", "⏷", "⏹", "⏺"] {
        let glyphs = drawn(symbol);
        assert_eq!(glyphs.len(), 1, "{symbol}: {glyphs:?}");
        let (real, color, family) = &glyphs[0];
        assert!(real, "{symbol} draws a missing glyph from {family}");
        assert!(!color, "{symbol} draws from the colour face {family}");
    }
}

#[test]
fn a_symbol_with_the_emoji_selector_draws_as_an_emoji() {
    let (_, shaper, _, _) = setup();
    if !installed(&shaper, "Noto Color Emoji") {
        eprintln!("no colour emoji face is installed");
        return;
    }
    let glyphs = drawn("⏸\u{FE0F}");
    assert!(
        glyphs.iter().any(|(real, color, _)| *real && *color),
        "{glyphs:?}"
    );
    let glyphs = drawn("😀");
    assert!(
        glyphs.iter().all(|(real, color, _)| *real && *color),
        "{glyphs:?}"
    );
}

#[test]
fn letters_still_draw_from_the_terminal_face() {
    let (_, shaper, _, _) = setup();
    if !installed(&shaper, "DejaVu Sans Mono") || installed(&shaper, "Geist Mono") {
        return;
    }
    for (real, color, family) in drawn("abc") {
        assert!(real && !color, "{family}");
        assert_eq!(family, "DejaVu Sans Mono");
    }
}

/// The face `text` is drawn from with `face`, in the family `family`, as plain or bold text.
fn face_of(family: &str, face: FaceSettings, bold: bool) -> (zgui::text::FaceId, bool) {
    let fonts = Fonts::system();
    let mut shaper = fonts.shaper();
    let resolved = families::resolve(&shaper, &[family.to_owned()], SIZE);
    let mut cache = ShapingCache::new(resolved, face);
    let metrics = CellMetrics {
        font_size: SIZE as u32,
        ..CellMetrics::default()
    };
    let style = RunFace {
        bold,
        ..RunFace::default()
    };
    let runs = cache.shape(&mut shaper, "abc", style, &metrics);
    (runs[0].face, runs[0].synthetic_bold > 0.0)
}

#[test]
fn the_configured_weights_choose_the_face_of_plain_and_bold_text() {
    let shaper = Fonts::system().shaper();
    if !installed(&shaper, "Adwaita Mono") {
        eprintln!("Adwaita Mono is not installed");
        return;
    }
    let normal = FaceSettings::default();
    let heavy = FaceSettings::of(&TerminalConfig {
        font_weight: 700,
        bold_weight: 400,
        ..TerminalConfig::default()
    });
    let regular = face_of("Adwaita Mono", normal.clone(), false);
    let bold = face_of("Adwaita Mono", normal, true);
    assert_ne!(regular, bold, "bold text draws from a heavier face");
    assert_eq!(face_of("Adwaita Mono", heavy.clone(), false), bold);
    assert_eq!(face_of("Adwaita Mono", heavy, true), regular);
}
