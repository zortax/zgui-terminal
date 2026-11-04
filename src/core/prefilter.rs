//! Where something can watch the byte stream before the parser sees it.
//!
//! The parser this crate uses consumes application program commands without reporting them, and
//! passes on only the operating system commands it knows. Anything that needs the rest reads the
//! bytes first.
//!
//! The filter finds the semantic prompt marks of operating system command 133, which a shell
//! with shell integration writes around its prompt. Every byte still reaches the parser, which
//! ignores the sequence. The mark is reported after the bytes of its sequence, so the emulator
//! state at that moment is the state at the mark.

/// What the filter hands on: bytes for the parser, or a mark it found between them.
#[derive(Debug, PartialEq, Eq)]
pub enum Piece<'a> {
    /// Bytes for the parser.
    Bytes(&'a [u8]),
    /// A semantic prompt mark, which follows the bytes before it.
    Mark(PromptMark),
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
            if let Some(mark) = self.step(byte) {
                forward(Piece::Bytes(&bytes[from..=index]));
                forward(Piece::Mark(mark));
                from = index + 1;
            }
        }
        if from < bytes.len() {
            forward(Piece::Bytes(&bytes[from..]));
        }
    }

    /// Reads one byte. Answers the mark a command ending at this byte made.
    fn step(&mut self, byte: u8) -> Option<PromptMark> {
        const ESCAPE: u8 = 0x1b;
        const BELL: u8 = 0x07;
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
                    ESCAPE => State::Escape,
                    _ => State::Ground,
                };
                None
            }
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
    fn finish(&mut self) -> Option<PromptMark> {
        self.state = State::Ground;
        let mark = parse(&self.body);
        self.body.clear();
        mark
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
        let mut filter = Prefilter::new();
        let mut out = Vec::new();
        let mut marks = Vec::new();
        for chunk in chunks {
            filter.filter(chunk, &mut |piece| match piece {
                Piece::Bytes(bytes) => out.extend_from_slice(bytes),
                Piece::Mark(mark) => marks.push((out.len(), mark)),
            });
        }
        (out, marks)
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
}
