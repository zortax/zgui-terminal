//! Where the rows sit.
//!
//! The grid is the screen and nothing more: one row element per row of the screen, each at its
//! own whole-pixel place. Scrolling moves what the emulator puts on the screen rather than moving
//! this over a longer document, so nothing here has to agree with a scroll position.
//!
//! The rows are placed here rather than by an ordinary layout because a row has to land on a
//! whole device pixel and at exactly its own multiple of the cell height. A column of boxes whose
//! heights are rounded one at a time drifts, and a drifting grid is a grid whose box drawing
//! shows a seam every few rows.

use std::cell::RefCell;
use std::rc::Rc;

use zgui::custom::{CustomElement, CustomLayoutCx, CustomMeasured, ScenePainter, Space};
use zgui::geom::{DevicePx, Point, Rect, Size};

use crate::config::{ColorPalette, Rgba};
use crate::render::metrics::CellMetrics;
use crate::render::theme::to_color;

/// How wide the bar showing where the screen sits in the history is, in device pixels.
const BAR_WIDTH: f32 = 8.0;

/// The least a thumb is drawn as, so a long history still leaves something to see and to grab.
const MIN_THUMB: f32 = 24.0;

/// Where the grid puts its rows.
pub struct GridLayout {
    /// How large a cell is.
    pub metrics: CellMetrics,
    /// How wide the grid is.
    pub columns: u16,
    /// How many rows the screen has.
    pub screen_lines: u16,
    /// The space around the rows.
    pub padding: u32,
    /// The colour behind every cell.
    pub background: Rgba,
    /// How many lines of history sit above the screen.
    pub history: usize,
    /// How many lines above the newest one the screen starts.
    pub display_offset: usize,
    /// The colour the bar's thumb is drawn in.
    pub thumb: Rgba,
}

impl GridLayout {
    /// Where the row at `row` sits, down from the top of the grid.
    pub fn row_y(&self, row: u16) -> u32 {
        self.padding + self.metrics.height * u32::from(row)
    }

    /// How tall the rows are together, with the space around them.
    pub fn content_height(&self) -> u32 {
        self.padding * 2 + self.metrics.height * u32::from(self.screen_lines)
    }

    /// Whether there is any history to show a bar for.
    pub fn has_history(&self) -> bool {
        self.history > 0
    }

    /// Where the bar's thumb sits inside a grid `height` pixels tall, as a top and a height.
    ///
    /// Answers nothing when the whole of the history is on screen, which is when a bar would say
    /// nothing worth the room it takes.
    pub fn thumb(&self, height: f32) -> Option<(f32, f32)> {
        if !self.has_history() || height <= 0.0 {
            return None;
        }
        let total = (self.history + usize::from(self.screen_lines)) as f32;
        let shown = f32::from(self.screen_lines);
        let span = (shown / total).clamp(0.0, 1.0);
        let thumb = (height * span).max(MIN_THUMB).min(height);
        // Zero is the bottom, so the thumb is at the end of the bar when nothing is scrolled back.
        let from_top = (self.history - self.display_offset.min(self.history)) as f32;
        let position = (from_top / self.history.max(1) as f32).clamp(0.0, 1.0);
        Some(((height - thumb) * position, thumb))
    }

    /// How far back the screen would be if the thumb were dragged to `y` in a grid `height` tall.
    pub fn offset_at(&self, y: f32, height: f32) -> usize {
        let Some((_, thumb)) = self.thumb(height) else {
            return 0;
        };
        let travel = (height - thumb).max(1.0);
        let position = (y / travel).clamp(0.0, 1.0);
        let from_top = (position * self.history as f32).round() as usize;
        self.history.saturating_sub(from_top)
    }
}

/// What a grid says when a style sheet names a colour it draws with.
pub type OnPalette = Rc<dyn Fn(ColorPalette)>;

/// The element the rows are children of.
pub struct GridElement {
    layout: Rc<RefCell<GridLayout>>,
    /// What the configuration says, which the style sheet is read over.
    base: ColorPalette,
    /// Who to tell when the style sheet says something different.
    on_palette: Option<OnPalette>,
    /// What was read last time, so the same answer is reported once.
    read: Option<ColorPalette>,
}

impl GridElement {
    /// A grid laid out by `layout`.
    pub fn new(layout: Rc<RefCell<GridLayout>>) -> Self {
        Self {
            layout,
            base: ColorPalette::default(),
            on_palette: None,
            read: None,
        }
    }

    /// Reads the colours off the style sheet, over `base`, and reports them to `on_palette`.
    pub fn with_palette(mut self, base: ColorPalette, on_palette: OnPalette) -> Self {
        self.base = base;
        self.on_palette = Some(on_palette);
        self
    }

    /// Draws over `base` from now on, which a new configuration makes necessary.
    pub fn set_base(&mut self, base: ColorPalette) {
        if self.base != base {
            self.base = base;
            // What was read was read over the old ground, so it says nothing about the new one.
            self.read = None;
        }
    }
}

impl CustomElement for GridElement {
    fn layout(&mut self, cx: &mut CustomLayoutCx<'_>) -> CustomMeasured {
        // The colours a style sheet named. Read here because this is where the computed style is
        // handed over, and reported only when the answer changed, so a frame that says the same
        // thing costs nothing.
        if let Some(report) = self.on_palette.as_ref() {
            let palette = crate::render::palette::from_style(cx.style, &self.base);
            if self.read != Some(palette) {
                self.read = Some(palette);
                report(palette);
            }
        }

        let layout = self.layout.borrow();
        let metrics = layout.metrics;
        let row_width = metrics.row_width(layout.columns) as f32;
        let width = cx.known_width.unwrap_or(row_width).max(row_width);
        let height = cx
            .known_height
            .unwrap_or_else(|| layout.content_height() as f32);

        if cx.final_pass {
            // One child per row of the screen, in order, each at its own whole-pixel place.
            let count = cx
                .access
                .child_count()
                .min(usize::from(layout.screen_lines));
            let left = metrics.left_padding(width, layout.columns, layout.padding as f32) as f32;
            for index in 0..count {
                let known = (Some(row_width), Some(metrics.height as f32));
                let available = (
                    Space::Definite(row_width),
                    Space::Definite(metrics.height as f32),
                );
                cx.access.layout_child(index, known, available);
                cx.access
                    .place_child(index, left, layout.row_y(index as u16) as f32);
            }
        }

        CustomMeasured {
            width,
            height,
            ..CustomMeasured::default()
        }
    }

    fn paint(&mut self, painter: &mut ScenePainter<'_>) {
        let layout = self.layout.borrow();
        let size = painter.size();

        // The colour behind every cell. The rows draw over it, and each one leaves the cells it
        // has nothing to say about to this.
        painter.fill(
            Rect::new(Point::new(DevicePx(0.0), DevicePx(0.0)), size),
            0.0,
            to_color(layout.background),
        );

        // Where the screen sits in the history. Drawn here rather than by a scroll container,
        // because the emulator owns the scrolling and this only reports it.
        if let Some((top, thumb)) = layout.thumb(size.height.0) {
            let width = BAR_WIDTH.min(size.width.0);
            painter.fill(
                Rect::new(
                    Point::new(DevicePx(size.width.0 - width), DevicePx(top)),
                    Size::new(DevicePx(width), DevicePx(thumb)),
                ),
                width / 2.0,
                to_color(layout.thumb),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> GridLayout {
        GridLayout {
            metrics: CellMetrics {
                width: 9,
                height: 19,
                ..CellMetrics::default()
            },
            columns: 80,
            screen_lines: 40,
            padding: 6,
            background: Rgba::hex(0x1e1e2e),
            history: 0,
            display_offset: 0,
            thumb: Rgba::hex(0x585b70),
        }
    }

    #[test]
    fn a_row_sits_at_its_own_multiple_of_the_cell_height() {
        let layout = layout();
        assert_eq!(layout.row_y(0), 6);
        assert_eq!(layout.row_y(1), 6 + 19);
        assert_eq!(layout.row_y(39), 6 + 19 * 39);
    }

    #[test]
    fn the_grid_is_as_tall_as_its_screen() {
        let layout = layout();
        assert_eq!(layout.content_height(), 12 + 19 * 40);
    }

    #[test]
    fn a_grid_with_no_history_shows_no_bar() {
        let layout = layout();
        assert!(layout.thumb(760.0).is_none());
    }

    #[test]
    fn the_thumb_sits_at_the_end_of_the_bar_when_nothing_is_scrolled_back() {
        let mut layout = layout();
        layout.history = 960;
        let (top, thumb) = layout.thumb(760.0).expect("a thumb");
        assert!((top + thumb - 760.0).abs() < 1.0, "top {top} thumb {thumb}");
    }

    #[test]
    fn the_thumb_sits_at_the_start_when_the_oldest_line_is_shown() {
        let mut layout = layout();
        layout.history = 960;
        layout.display_offset = 960;
        let (top, _) = layout.thumb(760.0).expect("a thumb");
        assert!(top.abs() < 1.0, "top {top}");
    }

    #[test]
    fn a_long_history_still_leaves_a_thumb_worth_grabbing() {
        let mut layout = layout();
        layout.history = 100_000;
        let (_, thumb) = layout.thumb(760.0).expect("a thumb");
        assert!(thumb >= MIN_THUMB);
    }

    #[test]
    fn dragging_the_thumb_from_end_to_end_covers_the_whole_history() {
        let mut layout = layout();
        layout.history = 960;
        assert_eq!(layout.offset_at(0.0, 760.0), 960);
        assert_eq!(layout.offset_at(760.0, 760.0), 0);
    }

    #[test]
    fn dragging_a_bar_with_no_history_asks_for_nothing() {
        let layout = layout();
        assert_eq!(layout.offset_at(100.0, 760.0), 0);
    }
}
