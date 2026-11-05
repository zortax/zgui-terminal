//! A terminal that outlives the view drawing it.
//!
//! A [`Terminal`](crate::Terminal) built from a transport owns the emulator, so the program dies
//! when the view is unmounted. That is right for a terminal somebody closed, and wrong for one
//! that is merely off screen: an application that hides a whole workspace, or closes one window
//! of several, should not be killing shells.
//!
//! A `TerminalSession` is the emulator held apart from any view. It is started once, drawn by as
//! many views as ask for it over its life, and stops when it is dropped.
//!
//! ```no_run
//! # use zgui_terminal::{TerminalSession, TerminalConfig, transport::TerminalSize};
//! # fn example(transport: impl zgui_terminal::transport::Transport + 'static) {
//! let session = TerminalSession::start(
//!     Box::new(transport),
//!     TerminalConfig::default(),
//!     TerminalSize { columns: 80, lines: 24, cell_width: 8, cell_height: 16 },
//! )
//! .expect("the program starts");
//! // `Terminal(session = session.clone(), ..)` draws it. Unmounting that view leaves the
//! // program running; dropping the last `session` handle is what stops it.
//! # }
//! ```
//!
//! # What happens while nobody is drawing it
//!
//! The emulator keeps running, and its grid is the truth. What it *publishes* — the rows that
//! changed, the overlay, the pointer shape — is meaningless to nobody, so it is thrown away
//! rather than piled up: a program writing at speed into a terminal nobody is looking at must not
//! grow a queue without bound. The title is the one thing kept, because an application draws a
//! tab for a terminal it is not showing.
//!
//! When a view attaches it asks for a redraw, and the emulator publishes every row again. The
//! end of the program is kept too, and a view that attaches after the end hears of it.

use std::sync::{Arc, Mutex};

use crate::config::{SchemeColors, TerminalConfig};
use crate::core::TerminalCore;
use crate::core::command::TerminalCommand;
use crate::core::update::FrameUpdate;
use crate::transport::{ExitStatus, TerminalSize, Transport, TransportError};

/// A running terminal, held apart from any view.
///
/// Cloning one is cloning a handle: every clone is the same program. The program stops when the
/// last handle goes.
#[derive(Clone)]
pub struct TerminalSession {
    inner: Arc<Inner>,
}

struct Inner {
    core: TerminalCore,
    /// Where published frames go while a view is drawing this, and how the program ended.
    ///
    /// Behind a mutex because the thread that drains the emulator is not the interface thread.
    /// One lock holds both, so a view that attaches at the end hears of the end once.
    watcher: Mutex<Watch>,
    /// The last title the program asked for, so an application can label a terminal it is not
    /// drawing.
    title: Mutex<Option<String>>,
}

/// The view a session publishes to, and the end it keeps.
#[derive(Default)]
struct Watch {
    /// Where published frames go. Nothing while no view draws the session.
    sender: Option<flume::Sender<FrameUpdate>>,
    /// How the program ended, once it has.
    exited: Option<ExitStatus>,
    /// Counts the views that attached, so a view lets go of the session only while it is still
    /// the one drawing it.
    attached: u64,
}

impl TerminalSession {
    /// Starts `transport` and the emulator around it.
    ///
    /// # Errors
    ///
    /// When the program will not start, or a thread cannot be made.
    pub fn start(
        transport: Box<dyn Transport>,
        config: TerminalConfig,
        size: TerminalSize,
    ) -> Result<Self, TransportError> {
        let core = TerminalCore::start(transport, config, size)?;
        let updates = core.updates.clone();
        let session = Self {
            inner: Arc::new(Inner {
                core,
                watcher: Mutex::new(Watch::default()),
                title: Mutex::new(None),
            }),
        };

        // One thread for the session's life, so the queue is drained whether or not anybody is
        // drawing. A frame nobody wants is dropped here rather than kept: the emulator's grid is
        // the truth, and a view that attaches later asks for the whole of it again.
        let inner = Arc::downgrade(&session.inner);
        std::thread::Builder::new()
            .name("zgui-terminal-fanout".to_owned())
            .spawn(move || {
                for update in updates.iter() {
                    let Some(inner) = inner.upgrade() else {
                        return;
                    };
                    if let FrameUpdate::Title(title) = &update
                        && let Ok(mut held) = inner.title.lock()
                    {
                        held.clone_from(title);
                    }
                    let Ok(mut watch) = inner.watcher.lock() else {
                        return;
                    };
                    if let FrameUpdate::Exited(status) = &update {
                        watch.exited = Some(*status);
                    }
                    if let Some(sender) = watch.sender.as_ref()
                        && sender.send(update).is_err()
                    {
                        // The view went away between the look and the send.
                        watch.sender = None;
                    }
                }
            })
            .map_err(|error| TransportError::new(error.to_string()))?;

        Ok(session)
    }

    /// The last title the program asked for.
    #[must_use]
    pub fn title(&self) -> Option<String> {
        self.inner.title.lock().ok().and_then(|held| held.clone())
    }

    /// How the program ended, once it has.
    #[must_use]
    pub fn exit_status(&self) -> Option<ExitStatus> {
        self.inner
            .watcher
            .lock()
            .ok()
            .and_then(|watch| watch.exited)
    }

    /// Whether a view is drawing this.
    #[must_use]
    pub fn is_drawn(&self) -> bool {
        self.inner
            .watcher
            .lock()
            .is_ok_and(|watch| watch.sender.is_some())
    }

    /// Says frames go here now, and asks for every row again.
    ///
    /// Answers where they will arrive, and the number of this attachment for
    /// [`Self::unwatch_view`]. A second view attaching replaces the first, because one emulator
    /// cannot be typed into from two places. A program that has ended tells the new view so at
    /// once.
    pub(crate) fn watch(&self) -> (u64, flume::Receiver<FrameUpdate>) {
        let (sender, receiver) = flume::unbounded();
        let mut attached = 0;
        if let Ok(mut watch) = self.inner.watcher.lock() {
            if let Some(status) = watch.exited {
                let _ = sender.send(FrameUpdate::Exited(status));
            }
            watch.sender = Some(sender);
            watch.attached += 1;
            attached = watch.attached;
        }
        // Everything, and not only what changes from here: a view attaching has no rows at all.
        self.inner.core.send(TerminalCommand::Redraw);
        (attached, receiver)
    }

    /// Says the view of attachment `attached` stops drawing. A view that attached since keeps
    /// the session, so a view that mounts before the one it replaces unmounts keeps drawing.
    pub(crate) fn unwatch_view(&self, attached: u64) {
        if let Ok(mut watch) = self.inner.watcher.lock()
            && watch.attached == attached
        {
            watch.sender = None;
        }
    }

    /// Watches, answering only the rows, for a test that has no view.
    ///
    /// The same seam a view uses; a test cannot build one, and what a view does with a frame is
    /// not what is under test here.
    #[cfg(any(test, feature = "testing"))]
    #[must_use]
    pub fn watch_for_test(&self) -> flume::Receiver<Vec<crate::snapshot::RowSnapshot>> {
        let (_, frames) = self.watch();
        let (rows, seen) = flume::unbounded();
        std::thread::spawn(move || {
            for update in frames.iter() {
                if let crate::core::update::FrameUpdate::Rows { rows: changed, .. } = update
                    && rows.send(changed).is_err()
                {
                    return;
                }
            }
        });
        seen
    }

    /// Draws with the colours of `scheme` from now on, the other colours as they are.
    ///
    /// For a session no view draws: a view sends the colours its style sheet names. A program
    /// that asked to hear of colour scheme changes is told when the text or background changes.
    pub fn set_scheme(&self, scheme: SchemeColors) {
        self.send(TerminalCommand::SetScheme(Box::new(scheme)));
    }

    /// Says nobody is drawing it any more. The program carries on.
    pub fn unwatch(&self) {
        if let Ok(mut watch) = self.inner.watcher.lock() {
            watch.sender = None;
        }
    }

    /// The emulator, for the view that is drawing it.
    pub(crate) fn core(&self) -> &TerminalCore {
        &self.inner.core
    }

    /// Asks the emulator to do something.
    pub(crate) fn send(&self, command: TerminalCommand) {
        self.inner.core.send(command);
    }

    /// Which modes the program has turned on.
    ///
    /// The one question the view asks the emulator directly, because how a key or a click is
    /// encoded depends on it and waiting a frame for the answer would encode the last one.
    #[must_use]
    pub fn mode(&self) -> alacritty_terminal::term::TermMode {
        *self.inner.core.term.lock().mode()
    }
}

impl std::fmt::Debug for TerminalSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TerminalSession")
            .field("drawn", &self.is_drawn())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::TerminalSession;
    use crate::TerminalConfig;
    use crate::transport::{Channels, TerminalSize};

    fn session() -> TerminalSession {
        let (to_peer, _from_terminal) = flume::unbounded::<Vec<u8>>();
        let (_to_terminal, from_peer) = flume::unbounded::<Vec<u8>>();
        TerminalSession::start(
            Box::new(Channels::new(to_peer, from_peer)),
            TerminalConfig::default(),
            TerminalSize {
                columns: 80,
                lines: 24,
                cell_width: 8,
                cell_height: 16,
            },
        )
        .expect("the terminal starts")
    }

    #[test]
    fn a_view_that_unmounts_after_its_successor_mounted_leaves_the_session_drawn() {
        let session = session();
        let (first, _old) = session.watch();
        let (second, _new) = session.watch();
        session.unwatch_view(first);
        assert!(session.is_drawn(), "the newer view keeps drawing");
        session.unwatch_view(second);
        assert!(!session.is_drawn());
    }
}
