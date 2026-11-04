//! One row of the grid, as a retained element.
//!
//! A row is the unit of damage. A row that has not changed is replayed from what it drew last
//! time, without being asked again, so a moving cursor costs two rows and a full-screen program
//! costs the rows it rewrote.

use std::cell::RefCell;
use std::rc::Rc;

use zgui::custom::{CustomElement, CustomLayoutCx, CustomMeasured, ScenePainter};
use zgui::geom::{DevicePx, Point, Rect, Size};

use crate::config::{ColorPalette, Rgba};
use crate::render::boxchars::{Builtin, CellRect};
use crate::render::metrics::CellMetrics;
use crate::render::runs::{DecorationKind, RowContext, RowPieces};
use crate::render::shaping::{ShapingCache, cluster_columns};
use crate::render::theme::to_color;
use crate::snapshot::{RowOverlay, RowSnapshot};
use alacritty_terminal::vte::ansi::CursorShape;

/// What every row of one terminal shares.
pub struct RowShared {
    /// How large a cell is.
    pub metrics: CellMetrics,
    /// The colours in force.
    pub palette: ColorPalette,
    /// How wide the grid is.
    pub columns: u16,
    /// Whether box drawing is built rather than shaped.
    pub builtin_glyphs: bool,
    /// Whether a blinking cursor is currently shown.
    pub blink_visible: bool,
    /// What shapes the text, and what it shaped before.
    pub shaping: ShapingCache,
    /// The framework's shaper.
    pub shaper: Option<zgui::app::Shaper>,
}

impl RowShared {
    /// What a row works its pieces out against.
    ///
    /// Whether the terminal has focus comes from the overlay, because that is what the row is
    /// told when it changes.
    fn context(&self, focused: bool) -> RowContext<'_> {
        RowContext {
            palette: &self.palette,
            columns: self.columns,
            cell: (self.metrics.width, self.metrics.height),
            builtin_glyphs: self.builtin_glyphs,
            focused,
        }
    }
}

/// One row of the grid.
pub struct RowElement {
    /// What this row holds.
    row: RowSnapshot,
    /// What is drawn over it.
    overlay: RowOverlay,
    /// What every row shares.
    shared: Rc<RefCell<RowShared>>,
}

impl RowElement {
    /// A row drawing `row`.
    pub fn new(row: RowSnapshot, overlay: RowOverlay, shared: Rc<RefCell<RowShared>>) -> Self {
        Self {
            row,
            overlay,
            shared,
        }
    }

    /// Draws `row` from now on.
    pub fn set_row(&mut self, row: RowSnapshot) {
        self.row = row;
    }

    /// Draws `overlay` over the row from now on.
    pub fn set_overlay(&mut self, overlay: RowOverlay) {
        self.overlay = overlay;
    }

    /// What this row holds.
    pub fn row(&self) -> &RowSnapshot {
        &self.row
    }

    /// What is drawn over it.
    pub fn overlay(&self) -> &RowOverlay {
        &self.overlay
    }
}

impl CustomElement for RowElement {
    fn layout(&mut self, cx: &mut CustomLayoutCx<'_>) -> CustomMeasured {
        let shared = self.shared.borrow();
        CustomMeasured {
            width: cx
                .known_width
                .unwrap_or_else(|| shared.metrics.row_width(shared.columns) as f32),
            height: shared.metrics.height as f32,
            ..CustomMeasured::default()
        }
    }

    fn paint(&mut self, painter: &mut ScenePainter<'_>) {
        let mut shared = self.shared.borrow_mut();
        let focused = self.overlay.focused;
        let pieces = crate::render::runs::pieces(&self.row, &self.overlay, shared.context(focused));
        paint_row(painter, &mut shared, focused, &pieces);
    }
}

/// Draws one row's pieces.
///
/// The order is what a terminal has always used: the colours behind the text, then anything
/// built from paths, then the text, then the lines under it, and the cursor last.
fn paint_row(
    painter: &mut ScenePainter<'_>,
    shared: &mut RowShared,
    focused: bool,
    pieces: &RowPieces,
) {
    let metrics = shared.metrics;
    let cell_width = metrics.width as f32;
    let cell_height = metrics.height as f32;

    let rect = |x: f32, y: f32, width: f32, height: f32| {
        Rect::new(
            Point::new(DevicePx(x), DevicePx(y)),
            Size::new(DevicePx(width), DevicePx(height)),
        )
    };

    // The colours behind the text. Every edge is a whole pixel, so two cells of one colour meet
    // with nothing between them.
    for run in &pieces.backgrounds {
        painter.fill(
            rect(
                metrics.column_x(run.column) as f32,
                0.0,
                (metrics.width * u32::from(run.width)) as f32,
                cell_height,
            ),
            0.0,
            to_color(run.color),
        );
    }

    // A block cursor sits behind the character it is on.
    if let Some(cursor) = pieces.cursor
        && shared.blink_visible
    {
        paint_cursor(painter, shared, focused, cursor, pieces.cursor_color);
    }

    // The characters that have to fill their cell exactly.
    for run in &pieces.builtins {
        let x = metrics.column_x(run.column) as f32;
        let color = to_color(run.color);
        match &run.builtin {
            Builtin::Rects(rects) => {
                for CellRect {
                    x: rx,
                    y: ry,
                    width,
                    height,
                } in rects
                {
                    painter.fill(
                        rect(x + *rx as f32, *ry as f32, *width as f32, *height as f32),
                        0.0,
                        color,
                    );
                }
            }
            Builtin::Path(path) => {
                // Moved into the cell by a whole number of pixels, which leaves the outline
                // exactly where it was worked out and lets the same shape be reused for every
                // cell that draws it.
                let mut moved = path.clone();
                moved.apply_affine(zgui::elements::kurbo::Affine::translate((
                    f64::from(x),
                    0.0,
                )));
                painter.fill_path(moved, color);
            }
            Builtin::Shade(amount) => {
                painter.fill(
                    rect(x, 0.0, cell_width, cell_height),
                    0.0,
                    to_color(run.color.with_alpha(*amount)),
                );
            }
        }
    }

    // The text. The shaper is taken out for the length of this and put back, because shaping and
    // the cache that holds what was shaped both live in the same borrow.
    let baseline = metrics.baseline as f32;
    if let Some(mut shaper) = shared.shaper.take() {
        for run in &pieces.texts {
            let face = crate::render::shaping::RunFace {
                bold: run.bold,
                italic: run.italic,
                emoji: run.emoji,
            };
            let shaped = shared.shaping.shape(&mut shaper, &run.text, face, &metrics);
            let color = to_color(run.color);
            let origin_x = metrics.column_x(run.column) as f32;
            for owned in shaped.iter() {
                let placed = place_on_cells(owned, &run.text, &metrics, origin_x);
                painter.glyphs(
                    &placed.as_run(zgui::custom::PaintSlot(0)),
                    Point::new(DevicePx(0.0), DevicePx(baseline)),
                    color,
                );
            }
        }
        shared.shaper = Some(shaper);
    }

    // The lines under and through the text.
    for run in &pieces.decorations {
        paint_decoration(
            painter, &metrics, run.column, run.width, run.kind, run.color,
        );
    }

    // A cursor that is not a block sits over the character.
    if let Some(cursor) = pieces.cursor
        && shared.blink_visible
        && !is_block(cursor.shape, focused)
    {
        paint_cursor(painter, shared, focused, cursor, pieces.cursor_color);
    }
}

/// Puts every cluster of a shaped run back on the cell it belongs to.
///
/// A shaper measures text by its own advances, which drift away from the grid within a few
/// characters. Anchoring each cluster to its column puts the text back on the cells, and keeping
/// the shaper's offsets inside a cluster leaves an accent where the face put it and lets a
/// character wider than one cell draw across both of them.
///
/// The vertical offsets are measured from the run's own baseline, so the caller places the
/// baseline where the cell wants it.
fn place_on_cells(
    run: &zgui::custom::ShapedRunOwned,
    text: &str,
    metrics: &CellMetrics,
    origin_x: f32,
) -> zgui::custom::ShapedRunOwned {
    let mut placed = run.clone();
    let Some(first) = run.glyphs.first() else {
        return placed;
    };
    let columns = cluster_columns(run, text);
    let baseline = first.y;
    let mut cluster_start = first.x;
    let mut cluster_at = columns.first().copied();

    for (index, glyph) in placed.glyphs.iter_mut().enumerate() {
        let column = columns.get(index).copied().unwrap_or(0);
        if Some(column) != cluster_at {
            cluster_at = Some(column);
            cluster_start = run.glyphs[index].x;
        }
        let inside = run.glyphs[index].x - cluster_start;
        glyph.x = origin_x + (metrics.width * u32::from(column)) as f32 + inside;
        glyph.y = run.glyphs[index].y - baseline;
    }
    placed
}

/// Whether a cursor of this shape is drawn behind the character.
fn is_block(shape: CursorShape, focused: bool) -> bool {
    matches!(shape, CursorShape::Block) && focused
}

/// Draws the cursor.
fn paint_cursor(
    painter: &mut ScenePainter<'_>,
    shared: &RowShared,
    focused: bool,
    cursor: crate::snapshot::CursorSnapshot,
    color: Rgba,
) {
    let metrics = shared.metrics;
    let x = metrics.column_x(cursor.column) as f32;
    let width = metrics.width as f32;
    let height = metrics.height as f32;
    let color = to_color(color);
    let rect = |x: f32, y: f32, width: f32, height: f32| {
        Rect::new(
            Point::new(DevicePx(x), DevicePx(y)),
            Size::new(DevicePx(width), DevicePx(height)),
        )
    };

    // A terminal without focus draws the cursor as an outline, which is how a person sees at a
    // glance where the keys are going.
    if !focused {
        let thickness = 1.0;
        painter.stroke(
            rect(
                x + thickness / 2.0,
                thickness / 2.0,
                width - thickness,
                height - thickness,
            ),
            0.0,
            thickness,
            color,
        );
        return;
    }

    match cursor.shape {
        CursorShape::Block | CursorShape::HollowBlock => {
            painter.fill(rect(x, 0.0, width, height), 0.0, color);
        }
        CursorShape::Beam => {
            let thickness = (metrics.width as f32 / 8.0).round().max(1.0);
            painter.fill(rect(x, 0.0, thickness, height), 0.0, color);
        }
        CursorShape::Underline => {
            let thickness = metrics.underline_thickness.max(1) as f32;
            painter.fill(rect(x, height - thickness, width, thickness), 0.0, color);
        }
        CursorShape::Hidden => {}
    }
}

/// Draws one line under or through the text.
fn paint_decoration(
    painter: &mut ScenePainter<'_>,
    metrics: &CellMetrics,
    column: u16,
    columns: u16,
    kind: DecorationKind,
    color: Rgba,
) {
    let x = metrics.column_x(column) as f32;
    let width = (metrics.width * u32::from(columns)) as f32;
    let color = to_color(color);
    let thickness = metrics.underline_thickness.max(1) as f32;
    let rect = |x: f32, y: f32, width: f32, height: f32| {
        Rect::new(
            Point::new(DevicePx(x), DevicePx(y)),
            Size::new(DevicePx(width), DevicePx(height)),
        )
    };

    match kind {
        DecorationKind::Underline => {
            painter.fill(
                rect(x, metrics.underline_y as f32, width, thickness),
                0.0,
                color,
            );
        }
        DecorationKind::DoubleUnderline => {
            let gap = thickness.max(1.0);
            let first = metrics.underline_y as f32 - gap;
            let second = metrics.underline_y as f32 + gap;
            let bottom = metrics.height as f32 - thickness;
            painter.fill(rect(x, first.max(0.0), width, thickness), 0.0, color);
            painter.fill(rect(x, second.min(bottom), width, thickness), 0.0, color);
        }
        DecorationKind::DottedUnderline | DecorationKind::DashedUnderline => {
            // The pattern is measured from the start of the run every time, so it does not drift
            // between one cell and the next.
            let period = if kind == DecorationKind::DottedUnderline {
                (thickness * 2.0).max(2.0)
            } else {
                (metrics.width as f32 / 2.0).max(4.0)
            };
            let on = (period / 2.0).max(1.0);
            let mut at = 0.0;
            while at < width {
                let length = on.min(width - at);
                painter.fill(
                    rect(x + at, metrics.underline_y as f32, length, thickness),
                    0.0,
                    color,
                );
                at += period;
            }
        }
        DecorationKind::Undercurl => {
            // One wave for each cell, so the curve carries on across a run of them.
            let amplitude = thickness.max(1.0);
            let top = (metrics.underline_y as f32 - amplitude).max(0.0);
            let mut path = zgui::elements::kurbo::BezPath::new();
            let period = metrics.width as f32;
            let mut at = 0.0;
            path.move_to((f64::from(x), f64::from(top + amplitude)));
            while at < width {
                let half = period / 2.0;
                path.quad_to(
                    (f64::from(x + at + half / 2.0), f64::from(top)),
                    (f64::from(x + at + half), f64::from(top + amplitude)),
                );
                path.quad_to(
                    (
                        f64::from(x + at + half + half / 2.0),
                        f64::from(top + amplitude * 2.0),
                    ),
                    (f64::from(x + at + period), f64::from(top + amplitude)),
                );
                at += period;
            }
            // Back along the underside, so the wave has a thickness.
            path.line_to((f64::from(x + width), f64::from(top + amplitude + thickness)));
            let mut back = width;
            while back > 0.0 {
                let half = period / 2.0;
                path.quad_to(
                    (
                        f64::from(x + back - half / 2.0),
                        f64::from(top + amplitude * 2.0 + thickness),
                    ),
                    (
                        f64::from(x + back - half),
                        f64::from(top + amplitude + thickness),
                    ),
                );
                path.quad_to(
                    (
                        f64::from(x + back - half - half / 2.0),
                        f64::from(top + thickness),
                    ),
                    (
                        f64::from(x + back - period),
                        f64::from(top + amplitude + thickness),
                    ),
                );
                back -= period;
            }
            path.close_path();
            painter.fill_path(path, color);
        }
        DecorationKind::Strikeout => {
            let thickness = metrics.strikeout_thickness.max(1) as f32;
            painter.fill(
                rect(x, metrics.strikeout_y as f32, width, thickness),
                0.0,
                color,
            );
        }
    }
}
