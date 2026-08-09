//! Where copied text goes, and where a paste comes from.
//!
//! The framework's own clipboard is what an application normally supplies, through
//! [`PlatformClipboard`]. The trait stays because a terminal is also run in tests and on
//! machines with no desktop, and because an application may want to decide for itself what a
//! program is allowed to read.

use std::cell::RefCell;
use std::rc::Rc;

use zgui::prelude::ClipboardKind;

pub use crate::core::update::ClipboardReply;

/// Where copied text goes, and where a paste comes from.
///
/// Reading is answered through `reply` rather than returned, because a desktop answers a
/// clipboard read out of band.
pub trait Clipboard: 'static {
    /// Puts `text` on the clipboard `kind`.
    fn store(&self, kind: ClipboardKind, text: String);

    /// Reads the clipboard `kind`, answering through `reply`.
    ///
    /// For a program that asked over the wire, which is answered in the program's own encoding.
    fn load(&self, kind: ClipboardKind, reply: ClipboardReply);

    /// Reads the clipboard `kind`, answering `then`.
    ///
    /// For the terminal's own paste. `then` may run later, because a desktop answers a clipboard
    /// read out of band, and it may answer nothing when the clipboard is empty or holds no text.
    fn read_text(&self, kind: ClipboardKind, then: Box<dyn FnOnce(Option<String>) + 'static>);
}

/// A clipboard that does not leave the program.
///
/// What a terminal copies, it can paste. Nothing else on the desktop sees it. This is the
/// default, so the component works in a test with no window and on a machine with no desktop.
#[derive(Debug, Default)]
pub struct InProcessClipboard {
    standard: RefCell<String>,
    primary: RefCell<String>,
}

impl InProcessClipboard {
    /// An empty clipboard.
    pub fn new() -> Self {
        Self::default()
    }

    fn slot(&self, kind: ClipboardKind) -> &RefCell<String> {
        match kind {
            ClipboardKind::Primary => &self.primary,
            _ => &self.standard,
        }
    }
}

impl Clipboard for InProcessClipboard {
    fn store(&self, kind: ClipboardKind, text: String) {
        *self.slot(kind).borrow_mut() = text;
    }

    fn load(&self, kind: ClipboardKind, reply: ClipboardReply) {
        let text = self.slot(kind).borrow().clone();
        reply.answer(&text);
    }

    fn read_text(&self, kind: ClipboardKind, then: Box<dyn FnOnce(Option<String>) + 'static>) {
        let text = self.slot(kind).borrow().clone();
        then((!text.is_empty()).then_some(text));
    }
}

/// The desktop's clipboard, through the framework.
pub struct PlatformClipboard {
    clipboards: zgui::prelude::Clipboards,
}

impl PlatformClipboard {
    /// The desktop's clipboard, when one is running.
    pub fn new() -> Option<Self> {
        zgui::prelude::try_use_clipboard().map(|clipboards| Self { clipboards })
    }
}

impl Clipboard for PlatformClipboard {
    fn store(&self, kind: ClipboardKind, text: String) {
        self.clipboards.set_text(kind, text);
    }

    fn load(&self, kind: ClipboardKind, reply: ClipboardReply) {
        self.clipboards.read_text(kind, move |text| {
            reply.answer(text.as_deref().unwrap_or_default());
        });
    }

    fn read_text(&self, kind: ClipboardKind, then: Box<dyn FnOnce(Option<String>) + 'static>) {
        self.clipboards.read_text(kind, then);
    }
}

/// The clipboard an application did not choose: the desktop's, or one that stays inside.
pub fn default_clipboard() -> Rc<dyn Clipboard> {
    match PlatformClipboard::new() {
        Some(clipboard) => Rc::new(clipboard),
        None => Rc::new(InProcessClipboard::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::writer::WriteRequest;

    /// The bytes one write carried.
    fn sent_bytes(receiver: &flume::Receiver<WriteRequest>) -> Vec<u8> {
        match receiver.recv().expect("an answer") {
            WriteRequest::Bytes(bytes) => bytes,
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn what_goes_on_the_clipboard_comes_back() {
        let (sender, receiver) = flume::unbounded();
        let clipboard = InProcessClipboard::new();
        clipboard.store(ClipboardKind::Standard, "hello".to_owned());
        clipboard.load(
            ClipboardKind::Standard,
            ClipboardReply::new(
                Some(std::sync::Arc::new(|text: &str| text.to_owned())),
                sender,
            ),
        );
        assert_eq!(sent_bytes(&receiver), b"hello");
    }

    #[test]
    fn the_two_clipboards_are_kept_apart() {
        let clipboard = InProcessClipboard::new();
        clipboard.store(ClipboardKind::Standard, "one".to_owned());
        clipboard.store(ClipboardKind::Primary, "two".to_owned());
        assert_eq!(*clipboard.slot(ClipboardKind::Standard).borrow(), "one");
        assert_eq!(*clipboard.slot(ClipboardKind::Primary).borrow(), "two");
    }

    #[test]
    fn reading_text_answers_what_was_stored() {
        let clipboard = InProcessClipboard::new();
        clipboard.store(ClipboardKind::Standard, "kept".to_owned());
        let answered = std::rc::Rc::new(std::cell::RefCell::new(None));
        let seen = std::rc::Rc::clone(&answered);
        clipboard.read_text(
            ClipboardKind::Standard,
            Box::new(move |text| *seen.borrow_mut() = text),
        );
        assert_eq!(answered.borrow().as_deref(), Some("kept"));
    }

    #[test]
    fn reading_an_empty_clipboard_answers_nothing() {
        let clipboard = InProcessClipboard::new();
        let answered = std::rc::Rc::new(std::cell::Cell::new(false));
        let seen = std::rc::Rc::clone(&answered);
        clipboard.read_text(
            ClipboardKind::Standard,
            Box::new(move |text| seen.set(text.is_none())),
        );
        assert!(answered.get());
    }

    #[test]
    fn an_unanswered_read_answers_with_nothing() {
        let (sender, receiver) = flume::unbounded();
        let reply = ClipboardReply::new(
            Some(std::sync::Arc::new(|text: &str| format!("[{text}]"))),
            sender,
        );
        drop(reply);
        assert_eq!(sent_bytes(&receiver), b"[]");
    }

    #[test]
    fn a_read_is_answered_once() {
        let (sender, receiver) = flume::unbounded();
        let reply = ClipboardReply::new(
            Some(std::sync::Arc::new(|text: &str| text.to_owned())),
            sender,
        );
        reply.answer("first");
        assert_eq!(sent_bytes(&receiver), b"first");
        assert!(receiver.try_recv().is_err());
    }
}
