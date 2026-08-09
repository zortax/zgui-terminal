//! An embeddable terminal emulator for Zgui applications.
//!
//! The component draws a grid of cells and speaks to a program through a [`Transport`]. The
//! transport is the application's: a local pseudo terminal, a kubernetes exec stream, an SSH
//! channel, or a test script. Nothing in the emulator names a process or a file descriptor.
//!
//! # Quick start
//!
//! ```no_run
//! use zgui::prelude::*;
//! use zgui_terminal::{Terminal, TerminalProps};
//! use zgui_terminal::transport::ReadWrite;
//!
//! # fn example(reader: std::fs::File, writer: std::fs::File) -> impl IntoView {
//! view! {
//!     Terminal(transport = ReadWrite::new(reader, writer))
//! }
//! # }
//! ```
//!
//! # How it is put together
//!
//! ```text
//!   Transport thread ──bytes──┐
//!   Term events ──────────────┼──► pump thread ──updates──► UI thread
//!   UI commands ──────────────┘    (vte parsing,            (row signals,
//!                                   damage to rows)          painting)
//!                                        │
//!                                        └──replies──► writer thread ──► Transport
//! ```
//!
//! The pump owns the emulator state and parses escape sequences off the UI thread. It publishes
//! only the rows that changed, so a frame costs what changed rather than what is on screen. The
//! UI thread never blocks on the program: input goes onto a queue that the writer thread drains.
//!
//! # What it draws
//!
//! Each row is a custom element, which makes a row the unit of damage: a moving cursor repaints
//! two rows and leaves the rest replayed. Cells are integer device pixels, and box drawing,
//! block elements and powerline glyphs are drawn from paths built for the exact cell, so they
//! join with no seam at any scale.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod clipboard;
pub mod config;
mod core;
pub mod event;
pub mod input;
pub mod render;
pub mod scroll;
pub mod search;
pub mod selection;
pub mod snapshot;
pub mod testing;
pub mod transport;

mod handle;
mod view;

pub use crate::clipboard::{Clipboard, ClipboardReply, InProcessClipboard};
pub use crate::config::{ColorPalette, CursorConfig, TerminalConfig};
pub use crate::event::{Hyperlink, MouseShape, TerminalEvent};
pub use crate::handle::{ClearMode, TerminalHandle};
pub use crate::render::CellMetrics;
pub use crate::scroll::{ScrollRequest, ScrollState};
pub use crate::search::{SearchDirection, SearchOptions, SearchState, SearchStatus};
pub use crate::snapshot::{GridDimensions, RowSnapshot};
pub use crate::transport::{
    ExitStatus, OutputSink, TerminalSize, Transport, TransportError, TransportSource,
};
pub use crate::view::{KeyFilter, Terminal, TerminalProps};

/// What the emulator is built on, for an application that needs to name its types.
pub use alacritty_terminal;
