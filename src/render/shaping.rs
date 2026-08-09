//! Keeping shaped text between frames.
//!
//! A terminal draws the same text over and over: the prompt, the chrome of a full-screen
//! program, every row that scrolls back into view. Shaping is the expensive part of drawing it,
//! and none of that work changes between one frame and the next, so it is kept.

use rustc_hash::FxHashMap;
use zgui::app::Shaper;
use zgui::custom::ShapedRunOwned;
use zgui_interned::Ident;

use crate::render::CellMetrics;

/// How many entries the cache holds before it starts letting go.
const CAPACITY: usize = 8192;

/// What decides whether two pieces of text shape the same way.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
struct RunKey {
    text: compact_str::CompactString,
    bold: bool,
    italic: bool,
    size: u32,
    generation: u32,
}

/// One shaped run, and when it was last drawn.
struct Entry {
    runs: std::rc::Rc<Vec<ShapedRunOwned>>,
    used: u64,
}

/// Shapes text, and keeps what it shaped.
pub struct ShapingCache {
    entries: FxHashMap<RunKey, Entry>,
    families: Vec<Ident>,
    letter_spacing: f32,
    /// Counts the frames, so the least recently drawn entries are the ones let go.
    clock: u64,
    hits: u64,
    misses: u64,
}

impl ShapingCache {
    /// A cache that shapes with `families`, which are already resolved to faces that exist.
    pub fn new(families: Vec<Ident>, letter_spacing: f32) -> Self {
        Self {
            entries: FxHashMap::default(),
            families,
            letter_spacing,
            clock: 0,
            hits: 0,
            misses: 0,
        }
    }

    /// Shapes with `families` from now on, and lets go of everything shaped before.
    pub fn set_families(&mut self, families: Vec<Ident>, letter_spacing: f32) {
        self.families = families;
        self.letter_spacing = letter_spacing;
        self.entries.clear();
    }

    /// The families in force.
    pub fn families(&self) -> &[Ident] {
        &self.families
    }

    /// Lets go of everything, which a change of size or scale makes necessary.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// How often the cache answered without shaping, and how often it had to shape.
    pub fn counts(&self) -> (u64, u64) {
        (self.hits, self.misses)
    }

    /// Starts a new frame.
    pub fn tick(&mut self) {
        self.clock += 1;
    }

    /// The runs `text` shapes into.
    pub fn shape(
        &mut self,
        shaper: &mut Shaper,
        text: &str,
        bold: bool,
        italic: bool,
        metrics: &CellMetrics,
    ) -> std::rc::Rc<Vec<ShapedRunOwned>> {
        let key = RunKey {
            text: text.into(),
            bold,
            italic,
            size: metrics.font_size,
            generation: metrics.generation,
        };
        let clock = self.clock;
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.used = clock;
            self.hits += 1;
            return std::rc::Rc::clone(&entry.runs);
        }

        self.misses += 1;
        let request = metrics.line_request(&self.families, bold, italic, self.letter_spacing);
        let runs = std::rc::Rc::new(shaper.shape_line(text, &request));
        if self.entries.len() >= CAPACITY {
            self.evict();
        }
        self.entries.insert(
            key,
            Entry {
                runs: std::rc::Rc::clone(&runs),
                used: clock,
            },
        );
        runs
    }

    /// Lets go of the entries that have gone longest without being drawn.
    fn evict(&mut self) {
        let mut ages: Vec<u64> = self.entries.values().map(|entry| entry.used).collect();
        ages.sort_unstable();
        // Half of them, so the cost of doing this is paid once rather than on every insert.
        let cutoff = ages.get(ages.len() / 2).copied().unwrap_or(0);
        self.entries.retain(|_, entry| entry.used > cutoff);
    }
}

/// Where one cluster of a shaped run is anchored.
///
/// A cluster is drawn from the cell its first character sits in, which is what keeps a row of
/// text on the grid. Inside a cluster the shaper's own offsets are kept, so an accent stays
/// where the face put it and a character wider than one cell still draws whole.
pub fn cluster_columns(run: &ShapedRunOwned, text: &str) -> Vec<u16> {
    let mut columns = Vec::with_capacity(run.glyphs.len());
    for cluster in &run.clusters {
        let byte = *cluster as usize;
        // Which cell the character at this byte belongs to, counting the cells before it.
        let column = text.get(..byte.min(text.len())).map(cells_in).unwrap_or(0);
        columns.push(column);
    }
    columns
}

/// How many cells `text` takes.
fn cells_in(text: &str) -> u16 {
    let mut cells = 0u16;
    for character in text.chars() {
        if is_zero_width(character) {
            continue;
        }
        cells += if is_wide(character) { 2 } else { 1 };
    }
    cells
}

/// Whether `character` takes two cells.
fn is_wide(character: char) -> bool {
    matches!(character as u32,
        0x1100..=0x115F | 0x2E80..=0x303E | 0x3041..=0x33FF
        | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xA000..=0xA4CF
        | 0xAC00..=0xD7A3 | 0xF900..=0xFAFF | 0xFE30..=0xFE6F
        | 0xFF00..=0xFF60 | 0xFFE0..=0xFFE6
        | 0x1F300..=0x1F64F | 0x1F900..=0x1F9FF | 0x20000..=0x3FFFD)
}

/// Whether `character` takes no room of its own.
fn is_zero_width(character: char) -> bool {
    matches!(character as u32,
        0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF
        | 0x20D0..=0x20FF | 0xFE00..=0xFE0F | 0xFE20..=0xFE2F | 0x200D)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_takes_one_cell_per_character() {
        assert_eq!(cells_in("hello"), 5);
    }

    #[test]
    fn a_wide_character_takes_two_cells() {
        assert_eq!(cells_in("世界"), 4);
        assert_eq!(cells_in("a世"), 3);
    }

    #[test]
    fn an_accent_takes_no_cell_of_its_own() {
        assert_eq!(cells_in("e\u{0301}"), 1);
    }

    #[test]
    fn a_cluster_is_anchored_at_the_cell_its_first_character_sits_in() {
        let run = ShapedRunOwned {
            face: zgui::custom::FaceId(0),
            size: 16.0,
            synthetic_bold: 0.0,
            synthetic_slant: 0.0,
            has_color: false,
            glyphs: Vec::new(),
            // Three glyphs, starting at bytes 0, 1 and 2 of "abc".
            clusters: vec![0, 1, 2],
        };
        assert_eq!(cluster_columns(&run, "abc"), vec![0, 1, 2]);
    }

    #[test]
    fn a_cluster_after_a_wide_character_is_anchored_two_cells_along() {
        let run = ShapedRunOwned {
            face: zgui::custom::FaceId(0),
            size: 16.0,
            synthetic_bold: 0.0,
            synthetic_slant: 0.0,
            has_color: false,
            glyphs: Vec::new(),
            // "世" is three bytes, so the next cluster starts at byte three.
            clusters: vec![0, 3],
        };
        assert_eq!(cluster_columns(&run, "世a"), vec![0, 2]);
    }
}
