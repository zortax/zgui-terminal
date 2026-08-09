//! Where something can watch the byte stream before the parser sees it.
//!
//! The parser this crate uses consumes application program commands without reporting them, and
//! passes on only the operating system commands it knows. Anything that needs the rest — the
//! kitty graphics protocol, sixel, the working directory an operating system command 7 carries —
//! reads the bytes first.
//!
//! Nothing does yet. The seam exists so that adding one changes this file alone: a filter keeps
//! its own buffer for a sequence split across two reads, reports what it found, and forwards
//! everything else.

/// Watches the byte stream on its way to the parser.
#[derive(Debug, Default)]
pub struct Prefilter {
    /// Kept for the state a filter carries between reads.
    _private: (),
}

impl Prefilter {
    /// A filter that forwards everything.
    pub fn new() -> Self {
        Self::default()
    }

    /// Passes `bytes` to `forward`, keeping what belongs to a sequence it is collecting.
    ///
    /// `forward` may be called any number of times for one chunk, and the pieces it receives join
    /// end to end into what the parser should see.
    pub fn filter(&mut self, bytes: &[u8], forward: &mut dyn FnMut(&[u8])) {
        if bytes.is_empty() {
            return;
        }
        forward(bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Everything that goes in comes out, whatever the chunks look like.
    fn forwards_unchanged(chunks: &[&[u8]]) {
        let mut filter = Prefilter::new();
        let mut out = Vec::new();
        for chunk in chunks {
            filter.filter(chunk, &mut |bytes| out.extend_from_slice(bytes));
        }
        assert_eq!(out, chunks.concat());
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
        // The split-boundary harness the later filter inherits: one stream, cut everywhere.
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
}
