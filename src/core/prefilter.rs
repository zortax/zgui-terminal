//! Where something can watch the byte stream before the parser sees it.
//!
//! The parser this crate uses consumes application program commands without reporting them, and
//! passes on only the operating system commands it knows. Anything that needs the rest reads the
//! bytes first.
//!
//! The filter finds the semantic prompt marks of operating system command 133, which a shell
//! with shell integration writes around its prompt. It also finds the controls the parser does
//! not know: the colour scheme reports of private mode 2031, the query for them, and a full
//! reset, which ends them. Every byte still reaches the parser. A mark or a control is reported
//! after the bytes of its sequence, so the emulator state at that moment is the state at it.

/// What the filter hands on: bytes for the parser, or a mark it found between them.
#[derive(Debug, PartialEq, Eq)]
pub enum Piece<'a> {
    /// Bytes for the parser.
    Bytes(&'a [u8]),
    /// A semantic prompt mark, which follows the bytes before it.
    Mark(PromptMark),
    /// A control the parser does not handle, which follows the bytes before it.
    Control(Control),
}

/// A control sequence the emulator answers outside the parser.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    /// The program turned the colour scheme reports on (`CSI ? 2031 h`) or off (`CSI ? 2031 l`).
    SchemeReports(bool),
    /// The program asked for the colour scheme (`CSI ? 996 n`).
    SchemeQuery,
    /// The program reset the terminal (`ESC c`).
    FullReset,
}

/// What a sequence that ends at a byte was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Found {
    Mark(PromptMark),
    Control(Control),
}

/// One semantic prompt mark.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptMark {
    /// A prompt starts at the cursor (`A`), or a continuation prompt does.
    PromptStart {
        /// Whether the shell draws the prompt again when the grid changes size.
        redraws: bool,
    },
    /// The text the person types starts at the cursor (`B`).
    InputStart,
    /// The output of a command starts at the cursor (`C`).
    OutputStart,
    /// A command ended (`D`).
    CommandEnd,
}

/// The longest command body the filter reads. A mark is short. A longer body is some other
/// command, and the filter stops reading it.
const BODY_LIMIT: usize = 64;

/// The longest control sequence the filter reads, without its introducer and final byte.
const CONTROL_LIMIT: usize = 32;

/// Where the filter is in the stream.
#[derive(Debug, Default)]
enum State {
    /// Outside any sequence the filter reads.
    #[default]
    Ground,
    /// After an escape.
    Escape,
    /// Inside an operating system command, with the body read so far.
    Command,
    /// Inside a command body, after an escape that can start its terminator.
    CommandEscape,
    /// Inside a control sequence, with its parameters read so far.
    Control,
}

/// Watches the byte stream on its way to the parser.
#[derive(Debug, Default)]
pub struct Prefilter {
    state: State,
    /// The body of the command being read.
    body: Vec<u8>,
}

impl Prefilter {
    /// A filter at the start of a stream.
    pub fn new() -> Self {
        Self::default()
    }

    /// Passes `bytes` to `forward`, with every mark found among them.
    ///
    /// `forward` may be called any number of times for one chunk. The bytes it receives join end
    /// to end into the chunk.
    pub fn filter(&mut self, bytes: &[u8], forward: &mut dyn FnMut(Piece<'_>)) {
        if bytes.is_empty() {
            return;
        }
        let mut from = 0;
        for (index, byte) in bytes.iter().copied().enumerate() {
            if let Some(found) = self.step(byte) {
                forward(Piece::Bytes(&bytes[from..=index]));
                forward(match found {
                    Found::Mark(mark) => Piece::Mark(mark),
                    Found::Control(control) => Piece::Control(control),
                });
                from = index + 1;
            }
        }
        if from < bytes.len() {
            forward(Piece::Bytes(&bytes[from..]));
        }
    }

    /// Reads one byte. Answers what a sequence ending at this byte was.
    fn step(&mut self, byte: u8) -> Option<Found> {
        const ESCAPE: u8 = 0x1b;
        const BELL: u8 = 0x07;
        const CANCEL: u8 = 0x18;
        const SUBSTITUTE: u8 = 0x1a;
        match self.state {
            State::Ground => {
                if byte == ESCAPE {
                    self.state = State::Escape;
                }
                None
            }
            State::Escape => {
                self.state = match byte {
                    b']' => {
                        self.body.clear();
                        State::Command
                    }
                    b'[' => {
                        self.body.clear();
                        State::Control
                    }
                    ESCAPE => State::Escape,
                    b'c' => {
                        self.state = State::Ground;
                        return Some(Found::Control(Control::FullReset));
                    }
                    _ => State::Ground,
                };
                None
            }
            State::Control => match byte {
                // Parameter and intermediate bytes.
                0x20..=0x3f => {
                    self.body.push(byte);
                    if self.body.len() > CONTROL_LIMIT {
                        self.state = State::Ground;
                    }
                    None
                }
                // The final byte.
                0x40..=0x7e => {
                    self.state = State::Ground;
                    let control = control(&self.body, byte);
                    self.body.clear();
                    control.map(Found::Control)
                }
                ESCAPE => {
                    self.state = State::Escape;
                    None
                }
                CANCEL | SUBSTITUTE => {
                    self.state = State::Ground;
                    None
                }
                // Other control characters act inside a sequence without ending it.
                _ => None,
            },
            State::Command => match byte {
                BELL => self.finish(),
                ESCAPE => {
                    self.state = State::CommandEscape;
                    None
                }
                _ => {
                    self.body.push(byte);
                    if self.body.len() > BODY_LIMIT || !b"133;".starts_with(self.prefix()) {
                        self.state = State::Ground;
                    }
                    None
                }
            },
            State::CommandEscape => {
                if byte == b'\\' {
                    self.finish()
                } else {
                    // Another sequence cuts the command short, and starts at the escape.
                    self.state = State::Escape;
                    self.step(byte)
                }
            }
        }
    }

    /// The part of the body the prefix check reads.
    fn prefix(&self) -> &[u8] {
        &self.body[..self.body.len().min(4)]
    }

    /// Ends the command being read, and answers the mark it was.
    fn finish(&mut self) -> Option<Found> {
        self.state = State::Ground;
        let mark = parse(&self.body);
        self.body.clear();
        mark.map(Found::Mark)
    }
}

/// The control a control sequence with `parameters` and `last` is, when the filter answers it.
fn control(parameters: &[u8], last: u8) -> Option<Control> {
    let private = parameters.strip_prefix(b"?")?;
    match last {
        b'h' | b'l' => private
            .split(|byte| *byte == b';')
            .any(|mode| mode == b"2031")
            .then_some(Control::SchemeReports(last == b'h')),
        b'n' if private == b"996" => Some(Control::SchemeQuery),
        _ => None,
    }
}

/// The mark the body of an operating system command is, when it is one.
fn parse(body: &[u8]) -> Option<PromptMark> {
    let rest = body.strip_prefix(b"133;")?;
    let mut fields = rest.split(|byte| *byte == b';');
    let kind = fields.next()?;
    match kind {
        b"A" | b"N" | b"P" => {
            let redraws = !fields.any(|field| field == b"redraw=0");
            Some(PromptMark::PromptStart { redraws })
        }
        b"B" | b"I" => Some(PromptMark::InputStart),
        b"C" => Some(PromptMark::OutputStart),
        b"D" => Some(PromptMark::CommandEnd),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a stream cut into `chunks` comes out as: the bytes, and each mark with how many
    /// bytes came before it.
    fn read(chunks: &[&[u8]]) -> (Vec<u8>, Vec<(usize, PromptMark)>) {
        let (out, marks, _) = read_all(chunks);
        (out, marks)
    }

    /// Bytes, marks and controls, each piece with how many bytes came before it.
    type Read = (Vec<u8>, Vec<(usize, PromptMark)>, Vec<(usize, Control)>);

    /// What a stream cut into `chunks` comes out as, controls included.
    fn read_all(chunks: &[&[u8]]) -> Read {
        let mut filter = Prefilter::new();
        let mut out = Vec::new();
        let mut marks = Vec::new();
        let mut controls = Vec::new();
        for chunk in chunks {
            filter.filter(chunk, &mut |piece| match piece {
                Piece::Bytes(bytes) => out.extend_from_slice(bytes),
                Piece::Mark(mark) => marks.push((out.len(), mark)),
                Piece::Control(control) => controls.push((out.len(), control)),
            });
        }
        (out, marks, controls)
    }

    /// The controls a stream holds, in order.
    fn controls(chunks: &[&[u8]]) -> Vec<Control> {
        let (out, _, controls) = read_all(chunks);
        assert_eq!(out, chunks.concat());
        controls.into_iter().map(|(_, control)| control).collect()
    }

    /// Everything that goes in comes out, whatever the chunks look like.
    fn forwards_unchanged(chunks: &[&[u8]]) {
        assert_eq!(read(chunks).0, chunks.concat());
    }

    #[test]
    fn plain_text_arrives_unchanged() {
        forwards_unchanged(&[b"hello world"]);
    }

    #[test]
    fn escape_sequences_arrive_unchanged() {
        forwards_unchanged(&[b"\x1b[31mred\x1b[0m\x1b]0;title\x07"]);
    }

    #[test]
    fn an_empty_chunk_forwards_nothing() {
        let mut filter = Prefilter::new();
        let mut calls = 0;
        filter.filter(b"", &mut |_| calls += 1);
        assert_eq!(calls, 0);
    }

    #[test]
    fn every_split_of_one_stream_gives_the_same_bytes() {
        let stream: &[u8] = b"\x1b_Gf=100,a=T;payload\x1b\\text\x1b[1;31mmore\x1bPq#0;2\x1b\\end";
        for cut in 0..=stream.len() {
            let (head, tail) = stream.split_at(cut);
            forwards_unchanged(&[head, tail]);
        }
    }

    #[test]
    fn a_stream_cut_into_single_bytes_gives_the_same_bytes() {
        let stream: &[u8] = b"\x1b_Gpayload\x1b\\rest";
        let chunks: Vec<&[u8]> = stream.chunks(1).collect();
        forwards_unchanged(&chunks);
    }

    #[test]
    fn a_prompt_mark_follows_the_bytes_of_its_sequence() {
        let stream = b"out\r\n\x1b]133;A\x07$ \x1b]133;B\x1b\\";
        let (bytes, marks) = read(&[stream]);
        assert_eq!(bytes, stream);
        assert_eq!(
            marks,
            vec![
                (5 + 8, PromptMark::PromptStart { redraws: true }),
                (stream.len(), PromptMark::InputStart),
            ]
        );
    }

    #[test]
    fn a_mark_split_anywhere_is_still_found_once() {
        let stream: &[u8] = b"x\x1b]133;C\x07y\x1b]133;D;0\x1b\\z";
        for cut in 0..=stream.len() {
            let (head, tail) = stream.split_at(cut);
            let (bytes, marks) = read(&[head, tail]);
            assert_eq!(bytes, stream);
            let kinds: Vec<PromptMark> = marks.into_iter().map(|(_, mark)| mark).collect();
            assert_eq!(
                kinds,
                vec![PromptMark::OutputStart, PromptMark::CommandEnd],
                "cut at {cut}"
            );
        }
    }

    #[test]
    fn a_prompt_that_is_not_redrawn_says_so() {
        let (_, marks) = read(&[b"\x1b]133;A;redraw=0;k=s\x07"]);
        assert_eq!(marks[0].1, PromptMark::PromptStart { redraws: false });
    }

    #[test]
    fn other_commands_make_no_mark() {
        let (_, marks) =
            read(&[b"\x1b]0;133;A\x07\x1b]1337;A\x07\x1b]8;;http://x\x1b\\\x1b]13;A\x07"]);
        assert!(marks.is_empty(), "{marks:?}");
    }

    #[test]
    fn a_command_cut_short_by_another_is_read_from_the_new_one() {
        let (_, marks) = read(&[b"\x1b]133;\x1b]133;C\x07"]);
        assert_eq!(marks, vec![(14, PromptMark::OutputStart)]);
    }

    #[test]
    fn a_long_command_is_not_held() {
        let mut long = b"\x1b]52;c;".to_vec();
        long.extend(std::iter::repeat_n(b'a', 10_000));
        long.extend_from_slice(b"\x07\x1b]133;A\x07");
        let (bytes, marks) = read(&[&long]);
        assert_eq!(bytes, long);
        assert_eq!(marks.len(), 1);
    }

    #[test]
    fn the_scheme_report_mode_is_found_on_and_off() {
        assert_eq!(
            controls(&[b"\x1b[?2031h", b"\x1b[?2031l"]),
            vec![Control::SchemeReports(true), Control::SchemeReports(false)]
        );
    }

    #[test]
    fn the_scheme_report_mode_is_found_among_other_modes() {
        assert_eq!(
            controls(&[b"\x1b[?2004;2031;1004h"]),
            vec![Control::SchemeReports(true)]
        );
    }

    #[test]
    fn a_control_follows_the_bytes_of_its_sequence() {
        let stream = b"ab\x1b[?2031hc";
        let (_, _, controls) = read_all(&[stream]);
        assert_eq!(controls, vec![(10, Control::SchemeReports(true))]);
    }

    #[test]
    fn a_control_split_anywhere_is_still_found_once() {
        let stream: &[u8] = b"x\x1b[?2031hy\x1b[?996nz\x1bcw";
        for cut in 0..=stream.len() {
            let (head, tail) = stream.split_at(cut);
            assert_eq!(
                controls(&[head, tail]),
                vec![
                    Control::SchemeReports(true),
                    Control::SchemeQuery,
                    Control::FullReset
                ],
                "cut at {cut}"
            );
        }
    }

    #[test]
    fn other_controls_are_not_reported() {
        assert!(
            controls(&[
                b"\x1b[2031h\x1b[?20310h\x1b[?2031$p\x1b[?997;1n\x1b[6n\x1b[31m\x1b[?2004h"
            ])
            .is_empty()
        );
    }

    #[test]
    fn a_cancelled_or_overlong_control_is_dropped() {
        let mut long = b"\x1b[?".to_vec();
        long.extend(std::iter::repeat_n(b'1', 100));
        long.extend_from_slice(b";2031h");
        assert!(controls(&[b"\x1b[?20\x1831h", &long]).is_empty());
    }

    #[test]
    fn a_control_does_not_hide_a_mark_after_it() {
        let (_, marks, controls) = read_all(&[b"\x1b[?2031h\x1b]133;A\x07"]);
        assert_eq!(controls.len(), 1);
        assert_eq!(marks.len(), 1);
    }
}
