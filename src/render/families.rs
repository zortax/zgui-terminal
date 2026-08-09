//! Deciding which faces the cells are drawn in.
//!
//! A request names families one at a time, and each has to be a family this machine really has.
//! The generic names a style sheet uses — `monospace` and the rest — name no face, so a list
//! holding only those resolves to nothing and the terminal would measure a cell from the
//! fallback numbers and draw no text at all.
//!
//! What goes to the shaper is therefore the configured list with a set of families that are
//! commonly installed added after it. The shaper takes the first that exists, and the ones after
//! it still answer for a character the first cannot draw.

use zgui::app::{LineRequest, Shaper};
use zgui_interned::Ident;

/// The families tried after the configured ones.
///
/// Ordered by what a terminal is usually set in, ending with the faces that are almost always
/// present on each desktop.
const FALLBACKS: &[&str] = &[
    "JetBrains Mono",
    "JetBrainsMono Nerd Font",
    "Fira Code",
    "FiraCode Nerd Font",
    "Cascadia Mono",
    "Cascadia Code",
    "Source Code Pro",
    "Hack",
    "Hack Nerd Font",
    "Iosevka",
    "Menlo",
    "Consolas",
    "SF Mono",
    "Ubuntu Mono",
    "DejaVu Sans Mono",
    "Liberation Mono",
    "Noto Sans Mono",
    "Courier New",
    "monospace",
];

/// Whether `name` is a generic that names no face of its own.
fn is_generic(name: &str) -> bool {
    matches!(
        name,
        "monospace" | "serif" | "sans-serif" | "cursive" | "fantasy" | "system-ui" | "ui-monospace"
    )
}

/// The families to draw with, given what the application asked for.
///
/// The answer holds at least one family that this machine has, when it has any of them. An empty
/// answer means nothing resolved, and the caller draws with the fallback measurements.
pub fn resolve(shaper: &Shaper, wanted: &[String], size_device_px: f32) -> Vec<Ident> {
    let mut names: Vec<String> = Vec::with_capacity(wanted.len() + FALLBACKS.len());
    for name in wanted {
        if !names.iter().any(|held| held == name) {
            names.push(name.clone());
        }
    }
    for name in FALLBACKS {
        if !names.iter().any(|held| held == name) {
            names.push((*name).to_owned());
        }
    }

    let families: Vec<Ident> = names.iter().map(|name| Ident::new(name)).collect();
    if resolves(shaper, &families, size_device_px) {
        return families;
    }

    // Nothing in the list as a whole. Whichever one of them exists on its own is still better
    // than drawing no text.
    for (index, name) in names.iter().enumerate() {
        if is_generic(name) {
            continue;
        }
        let one = vec![families[index]];
        if resolves(shaper, &one, size_device_px) {
            return one;
        }
    }
    Vec::new()
}

/// Whether `families` names a face this machine has.
fn resolves(shaper: &Shaper, families: &[Ident], size_device_px: f32) -> bool {
    shaper
        .line_metrics(&request(families, size_device_px))
        .is_some()
}

/// A plain request, for asking whether a list resolves.
pub fn request(families: &[Ident], size_device_px: f32) -> LineRequest<'_> {
    LineRequest {
        families,
        weight: 400,
        italic: false,
        size_device_px,
        letter_spacing: 0.0,
        ligatures: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_generic_names_are_known_to_name_no_face() {
        assert!(is_generic("monospace"));
        assert!(is_generic("ui-monospace"));
        assert!(!is_generic("JetBrains Mono"));
    }

    #[test]
    fn the_fallbacks_end_with_families_a_desktop_usually_has() {
        assert!(FALLBACKS.contains(&"DejaVu Sans Mono"));
        assert!(FALLBACKS.contains(&"Liberation Mono"));
        assert!(FALLBACKS.contains(&"Noto Sans Mono"));
    }
}
