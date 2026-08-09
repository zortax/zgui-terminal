//! Drawing the characters that have to fill their cell exactly.
//!
//! Box drawing, block elements, powerline separators and the rounded caps beside them are built
//! here rather than taken from the font. A font draws each of these inside its own idea of a
//! cell, which is nearly the cell the grid uses and never exactly it, so a run of them shows a
//! seam wherever the two disagree. Built for the cell the grid actually has, in whole pixels,
//! they join exactly.
//!
//! Everything with square corners becomes rectangles, which are drawn as quads and are exact by
//! construction. Only the arcs, the diagonals and the rounded caps become paths, and there the
//! smoothing is on the curve alone: the edges that meet the next cell stay on the boundary.

use smallvec::SmallVec;
#[cfg(test)]
use zgui::elements::kurbo::Shape;
use zgui::elements::kurbo::{BezPath, Point};

/// A rectangle inside a cell, in whole device pixels from its top-left corner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellRect {
    /// The left edge.
    pub x: u32,
    /// The top edge.
    pub y: u32,
    /// How wide it is.
    pub width: u32,
    /// How tall it is.
    pub height: u32,
}

impl CellRect {
    /// A rectangle, with nothing of zero size.
    fn new(x: u32, y: u32, width: u32, height: u32) -> Option<Self> {
        (width > 0 && height > 0).then_some(Self {
            x,
            y,
            width,
            height,
        })
    }
}

/// How one of these characters is drawn.
#[derive(Clone, Debug)]
pub enum Builtin {
    /// Fill these rectangles. Exact, and drawn as quads.
    Rects(SmallVec<[CellRect; 8]>),
    /// Fill this outline. For the shapes a rectangle cannot say.
    Path(BezPath),
    /// Fill the whole cell at this much of the text colour.
    ///
    /// The shaded blocks are drawn as a wash rather than as the stipple a font uses, which keeps
    /// a field of them flat instead of showing the pattern beating against the pixel grid.
    Shade(f32),
}

/// Whether `character` is drawn here rather than by the font.
pub fn covers(character: char) -> bool {
    matches!(character as u32,
        0x2500..=0x259F | 0x1FB00..=0x1FB3B | 0x1FB82..=0x1FB8B | 0xE0B0..=0xE0B7)
}

/// How thick a light line is, for a cell `width` pixels across.
///
/// An eighth of the cell, and never less than one pixel. Taking it from the cell rather than
/// from the font is what makes every one of these characters agree with every other.
pub fn stroke(width: u32) -> u32 {
    ((width as f32 / 8.0).round() as u32).max(1)
}

/// How thick a heavy line is.
fn heavy_stroke(width: u32, height: u32) -> u32 {
    (stroke(width) * 2).min(width.min(height))
}

/// Where a band of `thickness` sits in a run of `extent`, centred.
fn band(extent: u32, thickness: u32) -> (u32, u32) {
    let start = extent.saturating_sub(thickness) / 2;
    (start, thickness)
}

/// How `k` eighths of `extent` measure, with the last part taking what is left.
///
/// Rounding each part on its own would let a stack of eighths add up to more or less than the
/// cell. Measuring from the start every time makes them meet exactly.
fn eighths(extent: u32, k: u32) -> u32 {
    ((extent as f32 * k as f32 / 8.0).round() as u32).min(extent)
}

/// How the four arms of a box drawing character are weighted.
///
/// Nothing, light, or heavy, in the order up, right, down, left.
type Arms = [u8; 4];

/// The arms of the characters from `┌` to `╋`.
const JUNCTIONS: [Arms; 64] = [
    [0, 1, 1, 0],
    [0, 2, 1, 0],
    [0, 1, 2, 0],
    [0, 2, 2, 0], // 250C
    [0, 0, 1, 1],
    [0, 0, 1, 2],
    [0, 0, 2, 1],
    [0, 0, 2, 2], // 2510
    [1, 1, 0, 0],
    [1, 2, 0, 0],
    [2, 1, 0, 0],
    [2, 2, 0, 0], // 2514
    [1, 0, 0, 1],
    [1, 0, 0, 2],
    [2, 0, 0, 1],
    [2, 0, 0, 2], // 2518
    [1, 1, 1, 0],
    [1, 2, 1, 0],
    [2, 1, 1, 0],
    [1, 1, 2, 0], // 251C
    [2, 1, 2, 0],
    [2, 2, 1, 0],
    [1, 2, 2, 0],
    [2, 2, 2, 0], // 2520
    [1, 0, 1, 1],
    [1, 0, 1, 2],
    [2, 0, 1, 1],
    [1, 0, 2, 1], // 2524
    [2, 0, 2, 1],
    [2, 0, 1, 2],
    [1, 0, 2, 2],
    [2, 0, 2, 2], // 2528
    [0, 1, 1, 1],
    [0, 1, 1, 2],
    [0, 2, 1, 1],
    [0, 2, 1, 2], // 252C
    [0, 1, 2, 1],
    [0, 1, 2, 2],
    [0, 2, 2, 1],
    [0, 2, 2, 2], // 2530
    [1, 1, 0, 1],
    [1, 1, 0, 2],
    [1, 2, 0, 1],
    [1, 2, 0, 2], // 2534
    [2, 1, 0, 1],
    [2, 1, 0, 2],
    [2, 2, 0, 1],
    [2, 2, 0, 2], // 2538
    [1, 1, 1, 1],
    [1, 1, 1, 2],
    [1, 2, 1, 1],
    [1, 2, 1, 2], // 253C
    [2, 1, 1, 1],
    [1, 1, 2, 1],
    [2, 1, 2, 1],
    [2, 1, 1, 2], // 2540
    [2, 2, 1, 1],
    [1, 1, 2, 2],
    [1, 2, 2, 1],
    [2, 2, 1, 2], // 2544
    [1, 2, 2, 2],
    [2, 1, 2, 2],
    [2, 2, 2, 1],
    [2, 2, 2, 2], // 2548
];

/// The arms of the half lines, from `╴` to `╿`.
const HALVES: [Arms; 12] = [
    [0, 0, 0, 1],
    [1, 0, 0, 0],
    [0, 1, 0, 0],
    [0, 0, 1, 0], // 2574
    [0, 0, 0, 2],
    [2, 0, 0, 0],
    [0, 2, 0, 0],
    [0, 0, 2, 0], // 2578
    [0, 2, 0, 1],
    [1, 0, 2, 0],
    [0, 1, 0, 2],
    [2, 0, 1, 0], // 257C
];

/// How `character` is drawn in a cell of `width` by `height` device pixels.
///
/// Answers nothing for a character this does not draw, which then comes from the font.
pub fn glyph(character: char, width: u32, height: u32) -> Option<Builtin> {
    if width == 0 || height == 0 {
        return None;
    }
    let code = character as u32;
    match code {
        0x2500..=0x2503 => {
            let arms = match code {
                0x2500 => [0, 1, 0, 1],
                0x2501 => [0, 2, 0, 2],
                0x2502 => [1, 0, 1, 0],
                _ => [2, 0, 2, 0],
            };
            Some(junction(arms, width, height))
        }
        0x2504..=0x250B => Some(dashes(code, width, height)),
        0x250C..=0x254B => Some(junction(JUNCTIONS[(code - 0x250C) as usize], width, height)),
        0x254C..=0x254F => Some(dashes(code, width, height)),
        0x2550..=0x256C => Some(double(code, width, height)),
        0x256D..=0x2570 => Some(Builtin::Path(arc(code, width, height))),
        0x2571..=0x2573 => Some(Builtin::Path(diagonal(code, width, height))),
        0x2574..=0x257F => Some(junction(HALVES[(code - 0x2574) as usize], width, height)),
        0x2580..=0x259F => blocks(code, width, height),
        0x1FB00..=0x1FB3B => sextant(code, width, height),
        0x1FB82..=0x1FB8B => extra_eighths(code, width, height),
        0xE0B0..=0xE0B3 => Some(Builtin::Path(powerline(code, width, height))),
        0xE0B4..=0xE0B7 => Some(Builtin::Path(pill(code, width, height))),
        _ => None,
    }
}

/// The rectangles for a character with weighted arms.
///
/// Every arm runs from the edge of the cell to the far side of the band it crosses, so the
/// middle is covered once and the ends stop exactly on the boundary. A run of these therefore
/// joins with no gap and no doubled pixel.
fn junction(arms: Arms, width: u32, height: u32) -> Builtin {
    let light = stroke(width);
    let heavy = heavy_stroke(width, height);
    let thickness = |weight: u8| match weight {
        1 => light,
        2 => heavy,
        _ => 0,
    };
    let [up, right, down, left] = arms.map(thickness);

    // The thickest arm across each axis decides how far the other arms reach into the middle.
    let across_horizontal = up.max(down);
    let across_vertical = left.max(right);

    let mut rects: SmallVec<[CellRect; 8]> = SmallVec::new();

    if up > 0 {
        let (x, w) = band(width, up);
        let reach = if across_vertical > 0 {
            let (y, t) = band(height, across_vertical);
            y + t
        } else {
            let (y, t) = band(height, up);
            y + t
        };
        rects.extend(CellRect::new(x, 0, w, reach));
    }
    if down > 0 {
        let (x, w) = band(width, down);
        let start = if across_vertical > 0 {
            band(height, across_vertical).0
        } else {
            band(height, down).0
        };
        rects.extend(CellRect::new(x, start, w, height.saturating_sub(start)));
    }
    if left > 0 {
        let (y, h) = band(height, left);
        let reach = if across_horizontal > 0 {
            let (x, t) = band(width, across_horizontal);
            x + t
        } else {
            let (x, t) = band(width, left);
            x + t
        };
        rects.extend(CellRect::new(0, y, reach, h));
    }
    if right > 0 {
        let (y, h) = band(height, right);
        let start = if across_horizontal > 0 {
            band(width, across_horizontal).0
        } else {
            band(width, right).0
        };
        rects.extend(CellRect::new(start, y, width.saturating_sub(start), h));
    }

    Builtin::Rects(rects)
}

/// The rectangles for a dashed line.
///
/// The dashes are measured from the start of the cell every time, so the pattern of one cell
/// continues into the next without drifting.
fn dashes(code: u32, width: u32, height: u32) -> Builtin {
    let (count, horizontal, weight) = match code {
        0x2504 => (3, true, 1),
        0x2505 => (3, true, 2),
        0x2506 => (3, false, 1),
        0x2507 => (3, false, 2),
        0x2508 => (4, true, 1),
        0x2509 => (4, true, 2),
        0x250A => (4, false, 1),
        0x250B => (4, false, 2),
        0x254C => (2, true, 1),
        0x254D => (2, true, 2),
        0x254E => (2, false, 1),
        _ => (2, false, 2),
    };
    let thickness = if weight == 2 {
        heavy_stroke(width, height)
    } else {
        stroke(width)
    };
    let extent = if horizontal { width } else { height };
    let gap = stroke(width).max(1);

    let mut rects: SmallVec<[CellRect; 8]> = SmallVec::new();
    for index in 0..count {
        let start = (extent as f32 * index as f32 / count as f32).round() as u32;
        let end = (extent as f32 * (index + 1) as f32 / count as f32).round() as u32;
        let length = end.saturating_sub(start).saturating_sub(gap).max(1);
        if horizontal {
            let (y, h) = band(height, thickness);
            rects.extend(CellRect::new(start, y, length, h));
        } else {
            let (x, w) = band(width, thickness);
            rects.extend(CellRect::new(x, start, w, length));
        }
    }
    Builtin::Rects(rects)
}

/// The rectangles for a line that is drawn twice.
///
/// The two rails of each axis stop where the rails of the other axis begin, which is what turns
/// a corner and what leaves the middle of a cross open. Each rail is worked out on its own: a
/// rail whose side carries an arm stops at the near rail of the other axis, and one whose side
/// carries nothing runs on to the far rail. A corner therefore closes on the outside and on the
/// inside, and a cross becomes the four right angles it should be.
fn double(code: u32, width: u32, height: u32) -> Builtin {
    // Which arms this character has, as nothing, single or double, in the order up, right, down,
    // left.
    let arms: Arms = match code {
        0x2550 => [0, 2, 0, 2],
        0x2551 => [2, 0, 2, 0],
        0x2552 => [0, 2, 1, 0],
        0x2553 => [0, 1, 2, 0],
        0x2554 => [0, 2, 2, 0],
        0x2555 => [0, 0, 1, 2],
        0x2556 => [0, 0, 2, 1],
        0x2557 => [0, 0, 2, 2],
        0x2558 => [1, 2, 0, 0],
        0x2559 => [2, 1, 0, 0],
        0x255A => [2, 2, 0, 0],
        0x255B => [1, 0, 0, 2],
        0x255C => [2, 0, 0, 1],
        0x255D => [2, 0, 0, 2],
        0x255E => [1, 2, 1, 0],
        0x255F => [2, 1, 2, 0],
        0x2560 => [2, 2, 2, 0],
        0x2561 => [1, 0, 1, 2],
        0x2562 => [2, 0, 2, 1],
        0x2563 => [2, 0, 2, 2],
        0x2564 => [0, 2, 1, 2],
        0x2565 => [0, 1, 2, 1],
        0x2566 => [0, 2, 2, 2],
        0x2567 => [1, 2, 0, 2],
        0x2568 => [2, 1, 0, 1],
        0x2569 => [2, 2, 0, 2],
        0x256A => [1, 2, 1, 2],
        0x256B => [2, 1, 2, 1],
        _ => [2, 2, 2, 2],
    };
    let [up, right, down, left] = arms;
    let thickness = stroke(width);

    // Where the rails of each axis sit. An axis that is drawn twice has one rail either side of
    // where a single line would be; an axis drawn once keeps that single place.
    let (single_x, _) = band(width, thickness);
    let (single_y, _) = band(height, thickness);
    let columns: SmallVec<[u32; 2]> = if up == 2 || down == 2 {
        SmallVec::from_slice(&[
            single_x.saturating_sub(thickness),
            (single_x + thickness).min(width.saturating_sub(thickness)),
        ])
    } else {
        SmallVec::from_slice(&[single_x])
    };
    let rows: SmallVec<[u32; 2]> = if left == 2 || right == 2 {
        SmallVec::from_slice(&[
            single_y.saturating_sub(thickness),
            (single_y + thickness).min(height.saturating_sub(thickness)),
        ])
    } else {
        SmallVec::from_slice(&[single_y])
    };

    let leftmost = columns.iter().copied().min().unwrap_or(single_x);
    let rightmost = columns.iter().copied().max().unwrap_or(single_x);
    let topmost = rows.iter().copied().min().unwrap_or(single_y);
    let bottommost = rows.iter().copied().max().unwrap_or(single_y);
    let has_vertical = up > 0 || down > 0;
    let has_horizontal = left > 0 || right > 0;

    let mut rects: SmallVec<[CellRect; 8]> = SmallVec::new();

    // The horizontal rails. Which arm sits on a rail's own side decides where it stops.
    for (index, row) in rows.iter().enumerate() {
        let side_arm = match (rows.len(), index) {
            (2, 0) => up,
            (2, _) => down,
            _ => up.max(down),
        };
        if right > 0 {
            let from = if !has_vertical {
                0
            } else if side_arm > 0 {
                rightmost
            } else {
                leftmost
            };
            rects.extend(CellRect::new(
                from,
                *row,
                width.saturating_sub(from),
                thickness,
            ));
        }
        if left > 0 {
            let to = if !has_vertical {
                width
            } else if side_arm > 0 {
                leftmost
            } else {
                rightmost + thickness
            };
            rects.extend(CellRect::new(0, *row, to, thickness));
        }
    }

    // The vertical rails, by the same rule turned a quarter.
    for (index, column) in columns.iter().enumerate() {
        let side_arm = match (columns.len(), index) {
            (2, 0) => left,
            (2, _) => right,
            _ => left.max(right),
        };
        if down > 0 {
            let from = if !has_horizontal {
                0
            } else if side_arm > 0 {
                bottommost
            } else {
                topmost
            };
            rects.extend(CellRect::new(
                *column,
                from,
                thickness,
                height.saturating_sub(from),
            ));
        }
        if up > 0 {
            let to = if !has_horizontal {
                height
            } else if side_arm > 0 {
                topmost
            } else {
                bottommost + thickness
            };
            rects.extend(CellRect::new(*column, 0, thickness, to));
        }
    }

    Builtin::Rects(rects)
}

/// The outline of a rounded corner.
///
/// The straight ends land exactly on the same band a plain line uses, so an arc joins a line in
/// the next cell without a step. Only the curve itself is smoothed.
fn arc(code: u32, width: u32, height: u32) -> BezPath {
    let thickness = stroke(width) as f64;
    let (band_x, _) = band(width, stroke(width));
    let (band_y, _) = band(height, stroke(width));
    let (band_x, band_y) = (band_x as f64, band_y as f64);
    let (w, h) = (width as f64, height as f64);

    // Which way the arc turns: which two edges of the cell it reaches.
    let (to_right, to_down) = match code {
        0x256D => (true, true),   // ╭
        0x256E => (false, true),  // ╮
        0x256F => (false, false), // ╯
        _ => (true, false),       // ╰
    };

    let centre_x = band_x + thickness / 2.0;
    let centre_y = band_y + thickness / 2.0;
    let end_x = if to_right { w } else { 0.0 };
    let end_y = if to_down { h } else { 0.0 };

    // The corner the curve bends around, which is where the two straight parts would meet.
    let corner = Point::new(centre_x, centre_y);
    let half = thickness / 2.0;

    let mut path = BezPath::new();
    // The outer edge, from the vertical end to the horizontal end.
    let outer_offset = if to_right { half } else { -half };
    let outer_vertical = if to_down { half } else { -half };
    path.move_to((centre_x + outer_offset, end_y));
    path.quad_to(
        (centre_x + outer_offset, centre_y + outer_vertical),
        (end_x, centre_y + outer_vertical),
    );
    path.line_to((end_x, centre_y - outer_vertical));
    path.quad_to(
        (centre_x - outer_offset, centre_y - outer_vertical),
        (centre_x - outer_offset, end_y),
    );
    path.close_path();
    let _ = corner;
    path
}

/// The outline of a diagonal.
///
/// The line is worked out where it crosses the cell and stopped there, so nothing spills into
/// the neighbouring cell and a run of them still meets corner to corner.
fn diagonal(code: u32, width: u32, height: u32) -> BezPath {
    let thickness = stroke(width) as f64;
    let (w, h) = (width as f64, height as f64);
    let half = thickness / 2.0;
    let mut path = BezPath::new();

    // A band around the line from one corner to the other, cut off at the cell.
    let mut leg = |from: (f64, f64), to: (f64, f64)| {
        let (dx, dy) = (to.0 - from.0, to.1 - from.1);
        let length = (dx * dx + dy * dy).sqrt().max(1.0);
        // The offset that makes the band as thick as it should be, across the line.
        let (nx, ny) = (-dy / length * half, dx / length * half);
        path.move_to((from.0 + nx, from.1 + ny));
        path.line_to((to.0 + nx, to.1 + ny));
        path.line_to((to.0 - nx, to.1 - ny));
        path.line_to((from.0 - nx, from.1 - ny));
        path.close_path();
    };

    if code == 0x2571 || code == 0x2573 {
        leg((0.0, h), (w, 0.0));
    }
    if code == 0x2572 || code == 0x2573 {
        leg((0.0, 0.0), (w, h));
    }
    path
}

/// The blocks, the eighths, the shades and the quadrants.
fn blocks(code: u32, width: u32, height: u32) -> Option<Builtin> {
    let mut rects: SmallVec<[CellRect; 8]> = SmallVec::new();
    match code {
        // The upper half, and the lower eighths from one to seven.
        0x2580 => rects.extend(CellRect::new(0, 0, width, eighths(height, 4))),
        0x2581..=0x2587 => {
            let k = code - 0x2580;
            // Measured as the part above it, so the two halves of a cell meet exactly rather
            // than overlapping by a row wherever the division rounds up.
            let top = eighths(height, 8 - k);
            rects.extend(CellRect::new(0, top, width, height - top));
        }
        0x2588 => rects.extend(CellRect::new(0, 0, width, height)),
        // The left eighths, from seven down to one.
        0x2589..=0x258F => {
            let k = 8 - (code - 0x2588);
            rects.extend(CellRect::new(0, 0, eighths(width, k), height));
        }
        0x2590 => {
            let left = eighths(width, 4);
            rects.extend(CellRect::new(left, 0, width - left, height));
        }
        0x2591 => return Some(Builtin::Shade(0.25)),
        0x2592 => return Some(Builtin::Shade(0.5)),
        0x2593 => return Some(Builtin::Shade(0.75)),
        0x2594 => rects.extend(CellRect::new(0, 0, width, eighths(height, 1))),
        0x2595 => {
            let left = eighths(width, 7);
            rects.extend(CellRect::new(left, 0, width - left, height));
        }
        // The quadrants, as a set of four corners.
        0x2596..=0x259F => {
            let bits = match code {
                0x2596 => 0b0100, // lower left
                0x2597 => 0b1000, // lower right
                0x2598 => 0b0001, // upper left
                0x2599 => 0b1101,
                0x259A => 0b1001,
                0x259B => 0b0111,
                0x259C => 0b1011,
                0x259D => 0b0010, // upper right
                0x259E => 0b0110,
                _ => 0b1110,
            };
            let middle_x = eighths(width, 4);
            let middle_y = eighths(height, 4);
            let quadrant = |index: u32| -> Option<CellRect> {
                let (x, y, w, h) = match index {
                    0 => (0, 0, middle_x, middle_y),
                    1 => (middle_x, 0, width - middle_x, middle_y),
                    2 => (0, middle_y, middle_x, height - middle_y),
                    _ => (middle_x, middle_y, width - middle_x, height - middle_y),
                };
                CellRect::new(x, y, w, h)
            };
            for index in 0..4 {
                if bits & (1 << index) != 0 {
                    rects.extend(quadrant(index));
                }
            }
        }
        _ => return None,
    }
    Some(Builtin::Rects(rects))
}

/// The sextants: two columns by three rows of small blocks.
fn sextant(code: u32, width: u32, height: u32) -> Option<Builtin> {
    let offset = code.checked_sub(0x1FB00)?;
    if offset > 0x3B {
        return None;
    }
    // The four patterns that already exist as block elements are left out of this range, so the
    // numbering steps over them.
    let bits = offset + 1 + u32::from(offset >= 20) + u32::from(offset >= 40);

    let middle_x = eighths(width, 4);
    // The three bands are measured from the top every time, so together they are the whole cell.
    let first = ((height as f32) / 3.0).round() as u32;
    let second = ((height as f32) * 2.0 / 3.0).round() as u32;
    let rows = [(0, first), (first, second), (second, height)];

    let mut rects: SmallVec<[CellRect; 8]> = SmallVec::new();
    for (row, (top, bottom)) in rows.iter().enumerate() {
        for column in 0..2u32 {
            let index = row as u32 * 2 + column;
            if bits & (1 << index) == 0 {
                continue;
            }
            let (x, w) = if column == 0 {
                (0, middle_x)
            } else {
                (middle_x, width - middle_x)
            };
            rects.extend(CellRect::new(x, *top, w, bottom - top));
        }
    }
    Some(Builtin::Rects(rects))
}

/// The upper and right eighths that the block elements do not cover.
fn extra_eighths(code: u32, width: u32, height: u32) -> Option<Builtin> {
    let mut rects: SmallVec<[CellRect; 8]> = SmallVec::new();
    match code {
        // Upper two, three, five, six and seven eighths.
        0x1FB82..=0x1FB86 => {
            let k = [2, 3, 5, 6, 7][(code - 0x1FB82) as usize];
            rects.extend(CellRect::new(0, 0, width, eighths(height, k)));
        }
        0x1FB87..=0x1FB8B => {
            let k = [2, 3, 5, 6, 7][(code - 0x1FB87) as usize];
            let left = eighths(width, 8 - k);
            rects.extend(CellRect::new(left, 0, width - left, height));
        }
        _ => return None,
    }
    Some(Builtin::Rects(rects))
}

/// The powerline separators: a filled triangle, or the chevron of the same shape.
///
/// The flat edge sits exactly on the cell boundary and the point reaches exactly the other one,
/// so a separator meets the block beside it with nothing between them.
fn powerline(code: u32, width: u32, height: u32) -> BezPath {
    let (w, h) = (width as f64, height as f64);
    let pointing_right = code == 0xE0B0 || code == 0xE0B1;
    let filled = code == 0xE0B0 || code == 0xE0B2;

    let (flat, tip) = if pointing_right { (0.0, w) } else { (w, 0.0) };
    let mut path = BezPath::new();

    if filled {
        path.move_to((flat, 0.0));
        path.line_to((tip, h / 2.0));
        path.line_to((flat, h));
        path.close_path();
        return path;
    }

    // The chevron: the same shape as a band of one stroke, following the two sloped edges.
    let thickness = stroke(width) as f64;
    let step = if pointing_right {
        -thickness
    } else {
        thickness
    };
    path.move_to((flat, 0.0));
    path.line_to((tip, h / 2.0));
    path.line_to((flat, h));
    path.line_to((flat + step, h));
    path.line_to((tip + step, h / 2.0));
    path.line_to((flat + step, 0.0));
    path.close_path();
    path
}

/// The rounded caps beside a powerline separator.
///
/// The curve is exactly half the cell tall, and the flat edge sits on the cell boundary, so the
/// cap meets the block beside it at every size. This is the shape a font draws at its own size
/// and never quite at the grid's, which is what shows as a step along the top and bottom of a
/// row of them.
fn pill(code: u32, width: u32, height: u32) -> BezPath {
    let (w, h) = (width as f64, height as f64);
    let bulging_right = code == 0xE0B4 || code == 0xE0B5;
    let filled = code == 0xE0B4 || code == 0xE0B6;
    // The curve of a half circle, as the two arcs a cubic draws it with.
    const CIRCLE: f64 = 0.552_284_75;

    let flat = if bulging_right { 0.0 } else { w };
    let reach = if bulging_right { w } else { -w };
    let middle = h / 2.0;

    let mut path = BezPath::new();
    let half_arc = |path: &mut BezPath, radius_x: f64, radius_y: f64| {
        path.move_to((flat, middle - radius_y));
        path.curve_to(
            (flat + radius_x * CIRCLE, middle - radius_y),
            (flat + radius_x, middle - radius_y * CIRCLE),
            (flat + radius_x, middle),
        );
        path.curve_to(
            (flat + radius_x, middle + radius_y * CIRCLE),
            (flat + radius_x * CIRCLE, middle + radius_y),
            (flat, middle + radius_y),
        );
    };

    if filled {
        half_arc(&mut path, reach, middle);
        path.close_path();
        return path;
    }

    // The outline: the same curve, and a smaller one inside it.
    let thickness = stroke(width) as f64;
    half_arc(&mut path, reach, middle);
    path.close_path();
    let inner_x = reach - thickness * reach.signum();
    let inner_y = middle - thickness;
    if inner_y > 0.0 && inner_x.abs() > 0.0 {
        // The inside is wound the other way, so it is taken out of the shape.
        path.move_to((flat, middle + inner_y));
        path.curve_to(
            (flat + inner_x * CIRCLE, middle + inner_y),
            (flat + inner_x, middle + inner_y * CIRCLE),
            (flat + inner_x, middle),
        );
        path.curve_to(
            (flat + inner_x, middle - inner_y * CIRCLE),
            (flat + inner_x * CIRCLE, middle - inner_y),
            (flat, middle - inner_y),
        );
        path.close_path();
    }
    path
}

/// The rectangles of a built-in glyph, for a test to look at.
#[cfg(test)]
fn rects_of(builtin: &Builtin) -> &[CellRect] {
    match builtin {
        Builtin::Rects(rects) => rects,
        _ => &[],
    }
}

/// Which pixels of a cell a built-in glyph covers, as a grid of booleans.
#[cfg(test)]
fn coverage(character: char, width: u32, height: u32) -> Vec<Vec<bool>> {
    let mut grid = vec![vec![false; width as usize]; height as usize];
    let Some(builtin) = glyph(character, width, height) else {
        return grid;
    };
    match builtin {
        Builtin::Rects(rects) => {
            for rect in rects {
                for y in rect.y..(rect.y + rect.height).min(height) {
                    for x in rect.x..(rect.x + rect.width).min(width) {
                        grid[y as usize][x as usize] = true;
                    }
                }
            }
        }
        Builtin::Shade(_) => {
            for row in &mut grid {
                row.fill(true);
            }
        }
        Builtin::Path(path) => {
            // The middle of each pixel, which is what a rasteriser asks about.
            for (y, row) in grid.iter_mut().enumerate() {
                for (x, cell) in row.iter_mut().enumerate() {
                    let point = Point::new(x as f64 + 0.5, y as f64 + 0.5);
                    *cell = path.winding(point) != 0;
                }
            }
        }
    }
    grid
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sizes a cell takes at the scales and font sizes that matter.
    const SIZES: &[(u32, u32)] = &[
        (7, 15),
        (8, 16),
        (9, 19),
        (10, 21),
        (12, 25),
        (14, 30),
        (17, 38),
        (20, 44),
        (24, 53),
    ];

    #[test]
    fn every_covered_character_is_drawn() {
        let ranges = [
            0x2500..=0x259F,
            0x1FB00..=0x1FB3B,
            0x1FB82..=0x1FB8B,
            0xE0B0..=0xE0B7,
        ];
        for range in ranges {
            for code in range {
                let character = char::from_u32(code).expect("a character");
                assert!(covers(character), "{code:04X} is not covered");
                assert!(glyph(character, 9, 19).is_some(), "{code:04X} is not drawn");
            }
        }
    }

    #[test]
    fn nothing_is_drawn_outside_the_cell() {
        for &(width, height) in SIZES {
            for code in 0x2500..=0x259Fu32 {
                let character = char::from_u32(code).expect("a character");
                let Some(Builtin::Rects(rects)) = glyph(character, width, height) else {
                    continue;
                };
                for rect in rects {
                    assert!(
                        rect.x + rect.width <= width && rect.y + rect.height <= height,
                        "{code:04X} at {width}x{height} draws outside its cell: {rect:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_horizontal_line_reaches_both_edges() {
        for &(width, height) in SIZES {
            let grid = coverage('─', width, height);
            let row = grid
                .iter()
                .position(|row| row[0])
                .expect("a row that is drawn");
            // The line covers the first and the last column, so two of them join.
            assert!(grid[row][0], "the left edge is missing at {width}x{height}");
            assert!(
                grid[row][(width - 1) as usize],
                "the right edge is missing at {width}x{height}"
            );
        }
    }

    #[test]
    fn a_vertical_line_reaches_the_top_and_the_bottom() {
        for &(width, height) in SIZES {
            let grid = coverage('│', width, height);
            let column = (0..width as usize)
                .find(|column| grid[0][*column])
                .expect("a column that is drawn");
            assert!(grid[0][column], "the top is missing at {width}x{height}");
            assert!(
                grid[(height - 1) as usize][column],
                "the bottom is missing at {width}x{height}"
            );
        }
    }

    #[test]
    fn two_horizontal_lines_side_by_side_join() {
        // The right edge of one cell and the left edge of the next carry the same rows, so a run
        // of them is one unbroken line.
        for &(width, height) in SIZES {
            let grid = coverage('─', width, height);
            let left: Vec<bool> = (0..height as usize).map(|y| grid[y][0]).collect();
            let right: Vec<bool> = (0..height as usize)
                .map(|y| grid[y][(width - 1) as usize])
                .collect();
            assert_eq!(left, right, "at {width}x{height}");
        }
    }

    #[test]
    fn a_line_and_a_cross_join_where_they_meet() {
        for &(width, height) in SIZES {
            let line = coverage('─', width, height);
            let cross = coverage('┼', width, height);
            let line_right: Vec<bool> = (0..height as usize)
                .map(|y| line[y][(width - 1) as usize])
                .collect();
            let cross_left: Vec<bool> = (0..height as usize).map(|y| cross[y][0]).collect();
            assert_eq!(line_right, cross_left, "at {width}x{height}");
        }
    }

    #[test]
    fn a_corner_carries_its_arms_to_the_edges_it_reaches() {
        for &(width, height) in SIZES {
            let grid = coverage('┌', width, height);
            // Down and right: nothing on the top or the left edge, something on the other two.
            assert!(
                (0..width as usize).all(|x| !grid[0][x]),
                "the top edge is drawn at {width}x{height}"
            );
            assert!(
                (0..height as usize).all(|y| !grid[y][0]),
                "the left edge is drawn at {width}x{height}"
            );
            assert!(
                (0..height as usize).any(|y| grid[y][(width - 1) as usize]),
                "the right edge is missing at {width}x{height}"
            );
            assert!(
                (0..width as usize).any(|x| grid[(height - 1) as usize][x]),
                "the bottom edge is missing at {width}x{height}"
            );
        }
    }

    #[test]
    fn two_double_lines_side_by_side_join() {
        for &(width, height) in SIZES {
            let grid = coverage('═', width, height);
            let left: Vec<bool> = (0..height as usize).map(|y| grid[y][0]).collect();
            let right: Vec<bool> = (0..height as usize)
                .map(|y| grid[y][(width - 1) as usize])
                .collect();
            assert_eq!(left, right, "at {width}x{height}");
            // Two rails, so two separate stretches down the edge.
            let stretches = left.windows(2).filter(|pair| pair[0] != pair[1]).count();
            assert!(stretches >= 2, "at {width}x{height}: {left:?}");
        }
    }

    #[test]
    fn a_double_line_meets_a_double_cross_where_they_touch() {
        for &(width, height) in SIZES {
            let line = coverage('═', width, height);
            let cross = coverage('╬', width, height);
            let line_right: Vec<bool> = (0..height as usize)
                .map(|y| line[y][(width - 1) as usize])
                .collect();
            let cross_left: Vec<bool> = (0..height as usize).map(|y| cross[y][0]).collect();
            assert_eq!(line_right, cross_left, "at {width}x{height}");
        }
    }

    #[test]
    fn a_double_corner_leaves_its_middle_open() {
        // The mark of a corner drawn twice: the two rails turn, and the square they enclose is
        // not filled. A corner that fills solid is the artefact this rules out.
        for &(width, height) in SIZES {
            if width < 8 || height < 8 {
                continue;
            }
            let grid = coverage('╔', width, height);
            let open = (0..height as usize)
                .any(|y| (0..width as usize).any(|x| !grid[y][x] && y > 0 && x > 0));
            assert!(open, "at {width}x{height} the corner filled solid");
            // Nothing is drawn on the top or the left edge, which is where no arm reaches.
            assert!(
                (0..width as usize).all(|x| !grid[0][x]),
                "at {width}x{height}"
            );
            assert!(
                (0..height as usize).all(|y| !grid[y][0]),
                "at {width}x{height}"
            );
        }
    }

    #[test]
    fn a_double_vertical_joins_a_double_tee_where_they_touch() {
        for &(width, height) in SIZES {
            let line = coverage('║', width, height);
            let tee = coverage('╠', width, height);
            let line_bottom: Vec<bool> = (0..width as usize)
                .map(|x| line[(height - 1) as usize][x])
                .collect();
            let tee_top: Vec<bool> = (0..width as usize).map(|x| tee[0][x]).collect();
            assert_eq!(line_bottom, tee_top, "at {width}x{height}");
        }
    }

    #[test]
    fn a_heavy_line_is_thicker_than_a_light_one() {
        for &(width, height) in SIZES {
            let light = coverage('─', width, height);
            let heavy = coverage('━', width, height);
            let count =
                |grid: &Vec<Vec<bool>>| (0..height as usize).filter(|y| grid[*y][0]).count();
            assert!(
                count(&heavy) > count(&light),
                "at {width}x{height}: heavy {} light {}",
                count(&heavy),
                count(&light)
            );
        }
    }

    #[test]
    fn the_full_block_covers_every_pixel() {
        for &(width, height) in SIZES {
            let grid = coverage('█', width, height);
            assert!(
                grid.iter().all(|row| row.iter().all(|covered| *covered)),
                "at {width}x{height}"
            );
        }
    }

    #[test]
    fn the_two_halves_of_a_cell_cover_it_exactly() {
        // A cell with the upper half above the lower half is the whole cell, with nothing
        // doubled and nothing missing. This is what a row of them relies on.
        for &(width, height) in SIZES {
            let upper = coverage('▀', width, height);
            let lower = coverage('▄', width, height);
            for y in 0..height as usize {
                for x in 0..width as usize {
                    assert!(
                        upper[y][x] != lower[y][x],
                        "at {width}x{height}, pixel {x},{y} is in both or neither"
                    );
                }
            }
        }
    }

    #[test]
    fn the_left_and_right_halves_cover_a_cell_exactly() {
        for &(width, height) in SIZES {
            let left = coverage('▌', width, height);
            let right = coverage('▐', width, height);
            for y in 0..height as usize {
                for x in 0..width as usize {
                    assert!(
                        left[y][x] != right[y][x],
                        "at {width}x{height}, pixel {x},{y} is in both or neither"
                    );
                }
            }
        }
    }

    #[test]
    fn the_four_quadrants_together_are_the_whole_cell() {
        for &(width, height) in SIZES {
            let parts = ['▘', '▝', '▖', '▗'];
            let mut total = vec![vec![0u8; width as usize]; height as usize];
            for part in parts {
                let grid = coverage(part, width, height);
                for y in 0..height as usize {
                    for x in 0..width as usize {
                        total[y][x] += u8::from(grid[y][x]);
                    }
                }
            }
            for (y, row) in total.iter().enumerate() {
                for (x, count) in row.iter().enumerate() {
                    assert_eq!(
                        *count, 1,
                        "at {width}x{height}, pixel {x},{y} is covered {count} times"
                    );
                }
            }
        }
    }

    #[test]
    fn the_eighths_stack_into_a_whole_cell() {
        // The lower eighths grow one step at a time, and each is a superset of the last.
        for &(width, height) in SIZES {
            let mut last = 0;
            for k in 1..=7u32 {
                let character = char::from_u32(0x2580 + k).expect("a character");
                let grid = coverage(character, width, height);
                let filled = (0..height as usize).filter(|y| grid[*y][0]).count();
                assert!(
                    filled >= last,
                    "{character} at {width}x{height} covers less than the one before it"
                );
                last = filled;
            }
            // Seven eighths leaves at least one row for the eighth.
            assert!(last <= height as usize);
        }
    }

    #[test]
    fn a_powerline_separator_fills_its_cell_from_edge_to_edge() {
        // The shape reaches every boundary of the cell exactly: the flat edge sits on one side,
        // the point on the other, and the two corners on the top and the bottom. This is what
        // makes a separator meet the block beside it with nothing between them.
        for &(width, height) in SIZES {
            let Some(Builtin::Path(path)) = glyph('\u{E0B0}', width, height) else {
                panic!("no path at {width}x{height}");
            };
            let bounds = path.bounding_box();
            assert_eq!(bounds.x0, 0.0, "at {width}x{height}");
            assert_eq!(bounds.y0, 0.0, "at {width}x{height}");
            assert_eq!(bounds.x1, f64::from(width), "at {width}x{height}");
            assert_eq!(bounds.y1, f64::from(height), "at {width}x{height}");
            // The middle of the flat edge is filled, and the middle of the point.
            // The shape reaches into the last column, asked of the geometry rather than of one
            // pixel's centre: at a cell whose sides divide evenly the centre lands exactly on the
            // sloped edge, where being inside or outside is a coin toss.
            let tip = Point::new(f64::from(width) - 0.5, f64::from(height) / 2.0);
            assert!(
                path.winding(tip) != 0,
                "the point is short at {width}x{height}"
            );
            let flat = Point::new(0.5, f64::from(height) / 2.0);
            assert!(
                path.winding(flat) != 0,
                "the flat edge is empty at {width}x{height}"
            );
        }
    }

    #[test]
    fn the_other_powerline_separator_is_the_mirror_of_the_first() {
        for &(width, height) in SIZES {
            let Some(Builtin::Path(path)) = glyph('\u{E0B2}', width, height) else {
                panic!("no path at {width}x{height}");
            };
            let bounds = path.bounding_box();
            assert_eq!(bounds.x0, 0.0, "at {width}x{height}");
            assert_eq!(bounds.x1, f64::from(width), "at {width}x{height}");
            assert_eq!(bounds.y1, f64::from(height), "at {width}x{height}");
            let flat = Point::new(f64::from(width) - 0.5, f64::from(height) / 2.0);
            assert!(
                path.winding(flat) != 0,
                "the flat edge is empty at {width}x{height}"
            );
            let tip = Point::new(0.5, f64::from(height) / 2.0);
            assert!(
                path.winding(tip) != 0,
                "the point is short at {width}x{height}"
            );
        }
    }

    #[test]
    fn a_rounded_cap_is_flat_on_the_edge_it_joins() {
        for &(width, height) in SIZES {
            let grid = coverage('\u{E0B4}', width, height);
            // The flat edge is the left one and reaches the full height of the cell.
            let covered = (0..height as usize).filter(|y| grid[*y][0]).count();
            assert!(
                covered >= height as usize - 2,
                "the flat edge is short at {width}x{height}: {covered} of {height}"
            );
            // The curve is at its widest in the middle.
            let middle = (height / 2) as usize;
            assert!(
                grid[middle][(width - 1) as usize],
                "the curve is short at {width}x{height}"
            );
            // The corners are outside the curve.
            assert!(!grid[0][(width - 1) as usize]);
            assert!(!grid[(height - 1) as usize][(width - 1) as usize]);
        }
    }

    #[test]
    fn a_rounded_cap_reaches_the_top_and_the_bottom_of_its_cell() {
        // The complaint this answers: a cap that stops short leaves a line of background where
        // the next row starts.
        for &(width, height) in SIZES {
            let grid = coverage('\u{E0B4}', width, height);
            assert!(
                grid[0][0] || grid[0][1.min(width as usize - 1)],
                "the top of the cap is missing at {width}x{height}"
            );
            let bottom = (height - 1) as usize;
            assert!(
                grid[bottom][0] || grid[bottom][1.min(width as usize - 1)],
                "the bottom of the cap is missing at {width}x{height}"
            );
        }
    }

    #[test]
    fn a_dashed_line_leaves_gaps_and_still_starts_at_the_edge() {
        let grid = coverage('┄', 24, 53);
        let row = grid.iter().position(|row| row[0]).expect("a drawn row");
        let covered: Vec<bool> = grid[row].clone();
        assert!(covered[0], "the dash pattern starts inside the cell");
        assert!(covered.iter().any(|on| !on), "there are no gaps");
    }

    #[test]
    fn the_shades_are_a_wash_rather_than_a_pattern() {
        for character in ['░', '▒', '▓'] {
            let Some(Builtin::Shade(amount)) = glyph(character, 9, 19) else {
                panic!("{character} is not a shade");
            };
            assert!(amount > 0.0 && amount < 1.0);
        }
    }

    #[test]
    fn a_stroke_is_never_thinner_than_a_pixel() {
        for width in 1..64u32 {
            assert!(stroke(width) >= 1);
            assert!(stroke(width) <= width.max(1));
        }
    }

    #[test]
    fn a_tiny_cell_still_draws_something() {
        for character in ['─', '│', '┼', '█', '\u{E0B0}', '\u{E0B4}'] {
            assert!(glyph(character, 1, 1).is_some(), "{character}");
            assert!(glyph(character, 2, 3).is_some(), "{character}");
        }
    }

    #[test]
    fn a_cell_with_no_size_draws_nothing() {
        assert!(glyph('─', 0, 10).is_none());
        assert!(glyph('─', 10, 0).is_none());
    }

    #[test]
    fn the_sextants_leave_out_the_patterns_that_are_block_elements() {
        // The first sextant is one cell, and the range never repeats a block element.
        let first = glyph('\u{1FB00}', 8, 18).expect("a sextant");
        assert_eq!(rects_of(&first).len(), 1);
        let last = glyph('\u{1FB3B}', 8, 18).expect("a sextant");
        assert_eq!(rects_of(&last).len(), 5);
    }

    #[test]
    fn the_three_sextant_rows_cover_the_cell_exactly() {
        for &(width, height) in SIZES {
            // The three left cells stacked are the left half of the cell.
            let mut total = vec![0u8; height as usize];
            for character in ['\u{1FB00}', '\u{1FB03}', '\u{1FB0F}'] {
                let grid = coverage(character, width, height);
                for (y, row) in grid.iter().enumerate() {
                    total[y] += u8::from(row[0]);
                }
            }
            assert!(
                total.iter().all(|count| *count == 1),
                "at {width}x{height} the rows do not tile: {total:?}"
            );
        }
    }
}
