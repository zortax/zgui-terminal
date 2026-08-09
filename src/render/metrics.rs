//! How large a cell is, and where everything inside it sits.
//!
//! Every length here is a whole number of device pixels. That is the whole of what makes box
//! drawing join without a seam: a cell boundary that lands between two pixels leaves a row of
//! pixels that belongs to neither side, and no amount of care inside the cell can put it back.
//! Whatever is left over after the cells are counted becomes padding, which is what a terminal
//! that fills its window does.

use zgui::app::{LineRequest, ResolvedLineMetrics};

/// How large a cell is, and where everything inside it sits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellMetrics {
    /// How wide one cell is, in device pixels.
    pub width: u32,
    /// How tall one cell is, in device pixels.
    pub height: u32,
    /// How far below the top of the cell the baseline sits.
    pub baseline: u32,
    /// How far below the top of the cell an underline sits.
    pub underline_y: u32,
    /// How thick an underline is.
    pub underline_thickness: u32,
    /// How far below the top of the cell a line through the text sits.
    pub strikeout_y: u32,
    /// How thick that line is.
    pub strikeout_thickness: u32,
    /// The size the glyphs are shaped at, in device pixels.
    pub font_size: u32,
    /// The scale the face was measured at, which says when it has to be measured again.
    pub scale: f32,
    /// Which set of measurements these are, so a cache knows when to let go.
    pub generation: u32,
}

impl Default for CellMetrics {
    fn default() -> Self {
        Self {
            width: 8,
            height: 16,
            baseline: 12,
            underline_y: 14,
            underline_thickness: 1,
            strikeout_y: 8,
            strikeout_thickness: 1,
            font_size: 13,
            scale: 1.0,
            generation: 0,
        }
    }
}

impl CellMetrics {
    /// The measurements for a face measured by `metrics`, at `scale`.
    ///
    /// `line_height` multiplies the face's own line height, and `letter_spacing` is added to
    /// every cell. Both are in the same units the size is.
    pub fn from_face(
        metrics: &ResolvedLineMetrics,
        size_device_px: f32,
        scale: f32,
        line_height: f32,
        letter_spacing: f32,
        generation: u32,
    ) -> Self {
        let face = &metrics.metrics;
        let advance = metrics.cell_advance.0 + letter_spacing * scale;
        let ascent = face.ascent.0;
        let descent = face.descent.0;
        let natural = ascent + descent + face.line_gap.0;

        // Rounding down and never below one: a cell of zero pixels draws nothing, and a cell that
        // rounds up leaves the last column outside the window.
        let width = (advance.floor().max(1.0)) as u32;
        let height = ((natural * line_height).floor().max(1.0)) as u32;

        // The face's own content sits in the middle of whatever the line height added.
        let leading = (height as f32 - (ascent + descent)) / 2.0;
        let baseline = (leading + ascent).round().clamp(1.0, height as f32) as u32;

        let size = zgui::geom::CssPx(size_device_px);
        let underline_thickness = face
            .underline_thickness_or_fallback(size)
            .0
            .round()
            .max(1.0) as u32;
        // The face measures the offset upward from the baseline, so an underline is negative.
        let underline_offset = face.underline_offset_or_fallback(size).0;
        let underline_y = (baseline as f32 - underline_offset).round().clamp(
            (baseline + 1) as f32,
            (height.saturating_sub(underline_thickness)).max(1) as f32,
        ) as u32;

        let strikeout_thickness = face
            .strikeout_thickness_or_fallback(size)
            .0
            .round()
            .max(1.0) as u32;
        let strikeout_offset = face.strikeout_offset_or_fallback(size).0;
        let strikeout_y = (baseline as f32 - strikeout_offset).round().clamp(
            1.0,
            (height.saturating_sub(strikeout_thickness)).max(1) as f32,
        ) as u32;

        Self {
            width,
            height,
            baseline,
            underline_y,
            underline_thickness,
            strikeout_y,
            strikeout_thickness,
            font_size: size_device_px.round().max(1.0) as u32,
            scale,
            generation,
        }
    }

    /// How many columns and lines fit in a space of `width` by `height` device pixels.
    ///
    /// `padding` is the least space to leave on each side. What the cells do not use is added to
    /// it, so the grid sits in the middle of the space it is given.
    pub fn grid_size(&self, width: f32, height: f32, padding: f32) -> (u16, u16) {
        let usable_width = (width - 2.0 * padding).max(0.0) as u32;
        let usable_height = (height - 2.0 * padding).max(0.0) as u32;
        let columns = (usable_width / self.width).max(2);
        let lines = (usable_height / self.height).max(1);
        (
            columns.min(u32::from(u16::MAX)) as u16,
            lines.min(u32::from(u16::MAX)) as u16,
        )
    }

    /// Where the grid starts inside a space of `width` device pixels holding `columns` columns.
    ///
    /// What the cells do not use is split between the two sides, and the answer is a whole
    /// number, so every column after it lands on a pixel.
    pub fn left_padding(&self, width: f32, columns: u16, padding: f32) -> u32 {
        let used = self.width * u32::from(columns);
        let spare = (width.max(0.0) as u32).saturating_sub(used);
        let least = padding.max(0.0) as u32;
        // The extra is shared, and the half that is left over goes on the right.
        (spare / 2).max(least.min(spare))
    }

    /// How wide `columns` cells are.
    pub fn row_width(&self, columns: u16) -> u32 {
        self.width * u32::from(columns)
    }

    /// Where column `column` starts.
    pub fn column_x(&self, column: u16) -> u32 {
        self.width * u32::from(column)
    }

    /// The request that shapes text at these measurements.
    pub fn line_request<'a>(
        &self,
        families: &'a [zgui_interned::Ident],
        bold: bool,
        italic: bool,
        letter_spacing: f32,
    ) -> LineRequest<'a> {
        LineRequest {
            families,
            weight: if bold { 700 } else { 400 },
            italic,
            size_device_px: self.font_size as f32,
            letter_spacing,
            // A ligature draws two characters as one glyph and takes one cell doing it, which is
            // exactly wrong for a grid.
            ligatures: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics() -> CellMetrics {
        CellMetrics {
            width: 9,
            height: 19,
            ..CellMetrics::default()
        }
    }

    #[test]
    fn the_grid_holds_as_many_whole_cells_as_fit() {
        let metrics = metrics();
        // Ten cells of nine pixels, with six of padding on each side.
        let (columns, lines) = metrics.grid_size(9.0 * 10.0 + 12.0, 19.0 * 4.0 + 12.0, 6.0);
        assert_eq!(columns, 10);
        assert_eq!(lines, 4);
    }

    #[test]
    fn a_space_too_small_still_holds_a_grid() {
        let metrics = metrics();
        let (columns, lines) = metrics.grid_size(1.0, 1.0, 6.0);
        assert_eq!(columns, 2);
        assert_eq!(lines, 1);
    }

    #[test]
    fn what_the_cells_do_not_use_becomes_padding() {
        let metrics = metrics();
        // Ninety-five pixels holds ten cells of nine, with five left over.
        let padding = metrics.left_padding(95.0, 10, 0.0);
        assert_eq!(padding, 2);
    }

    #[test]
    fn every_column_lands_on_a_whole_pixel() {
        let metrics = metrics();
        for column in 0..64u16 {
            assert_eq!(metrics.column_x(column), 9 * u32::from(column));
        }
    }

    #[test]
    fn the_underline_stays_inside_the_cell() {
        let metrics = CellMetrics {
            height: 19,
            baseline: 15,
            underline_y: 17,
            underline_thickness: 1,
            ..CellMetrics::default()
        };
        assert!(metrics.underline_y + metrics.underline_thickness <= metrics.height);
        assert!(metrics.underline_y > metrics.baseline);
    }

    #[test]
    fn a_row_is_exactly_as_wide_as_its_cells() {
        let metrics = metrics();
        assert_eq!(metrics.row_width(80), 9 * 80);
    }
}
