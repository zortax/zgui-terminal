//! Sending text a person pasted.

/// What a program wraps a paste in when it asked to be told.
const START: &[u8] = b"\x1b[200~";
/// What ends it.
const END: &[u8] = b"\x1b[201~";

/// The bytes for pasting `text`.
///
/// A newline becomes a carriage return, which is what a key press sends. The sequence that ends
/// a bracketed paste is removed whatever the mode, because text carrying it could otherwise end
/// the paste early and have the rest read as commands. Without bracketing, every escape goes
/// too: a program that did not ask to be told cannot tell text from a command.
pub fn encode(text: &str, bracketed: bool) -> Vec<u8> {
    let mut body = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '\n' => body.push('\r'),
            '\x1b' if !bracketed => {}
            other => body.push(other),
        }
    }
    // Whatever the mode: text that carries this would end the paste and leave the rest to be read
    // as commands.
    let body = body.replace("\x1b[201~", "");

    if !bracketed {
        return body.into_bytes();
    }
    let mut bytes = Vec::with_capacity(body.len() + START.len() + END.len());
    bytes.extend_from_slice(START);
    bytes.extend_from_slice(body.as_bytes());
    bytes.extend_from_slice(END);
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newlines_become_carriage_returns() {
        assert_eq!(encode("a\nb", false), b"a\rb");
    }

    #[test]
    fn a_bracketed_paste_is_wrapped() {
        assert_eq!(encode("hi", true), b"\x1b[200~hi\x1b[201~");
    }

    #[test]
    fn the_ending_sequence_is_removed_when_bracketed() {
        let bytes = encode("a\x1b[201~b", true);
        assert_eq!(bytes, b"\x1b[200~ab\x1b[201~");
    }

    #[test]
    fn escapes_are_removed_when_the_program_did_not_ask_to_be_told() {
        assert_eq!(encode("a\x1b[31mb", false), b"a[31mb");
    }

    #[test]
    fn escapes_are_kept_when_the_paste_is_bracketed() {
        assert_eq!(encode("a\x1bb", true), b"\x1b[200~a\x1bb\x1b[201~");
    }

    #[test]
    fn empty_text_still_brackets() {
        assert_eq!(encode("", true), b"\x1b[200~\x1b[201~");
        assert_eq!(encode("", false), b"");
    }
}
