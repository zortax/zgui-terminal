//! Turning the emulator's colours into the framework's.

use zgui::canvas::zgui_color::Color;

use crate::config::Rgba;

/// The framework's colour for `color`.
pub fn to_color(color: Rgba) -> Color {
    Color::srgb_u8(color.r, color.g, color.b, color.a)
}

/// Whether `color` is close enough to `other` that drawing one over the other shows nothing.
pub fn is_same(color: Rgba, other: Rgba) -> bool {
    color == other
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_colour_keeps_its_channels() {
        let color = to_color(Rgba::rgb(255, 128, 0));
        let [red, green, blue] = color.components();
        assert!((red - 1.0).abs() < 0.01);
        assert!((green - 128.0 / 255.0).abs() < 0.01);
        assert!(blue.abs() < 0.01);
        assert!((color.alpha() - 1.0).abs() < 0.01);
    }

    #[test]
    fn a_transparent_colour_keeps_its_opacity() {
        let color = to_color(Rgba::rgb(0, 0, 0).with_alpha(0.5));
        assert!((color.alpha() - 0.5).abs() < 0.01);
    }
}
