//! Turning what a person does into what a program reads.
//!
//! Every encoder here is a plain function of its inputs and the emulator's mode, which is what
//! lets the whole surface be tested without a window or a program.

pub mod keyboard;
pub mod mouse;
pub mod paste;

pub use crate::input::keyboard::{Bytes, KeyBindings, KeyState, encode_key};
pub use crate::input::mouse::{MouseReport, cell_at, encode as encode_mouse};
