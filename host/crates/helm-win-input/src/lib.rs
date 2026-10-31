//! # helm-win-input
//!
//! Windows input helpers for the Helm PC agent:
//!
//! * **Input injection** – [`InputEvent`] (the phone's `in` frames, ROADMAP §1.1) and the
//!   thread-safe [`Injector`] built on `SendInput`, with sub-pixel / sub-notch accumulation,
//!   held-key tracking and [`Injector::release_all`].
//! * **Text-focus hint** – [`TextFocusWatcher`] reports whether the focused element accepts
//!   typing, using UI Automation focus events.
//! * **Clipboard** – [`clipboard`] text and PNG images, plus [`ClipboardWatcher`].
//!
//! The pure parts ([`keys`], [`motion`], [`dib`], [`inject::Planner`], [`focus::is_text_target`])
//! build and test on every platform; the Windows parts return [`Error::Unsupported`] elsewhere.

pub mod clipboard;
pub mod dib;
mod error;
mod event;
pub mod focus;
pub mod inject;
pub mod keys;
pub mod motion;

#[cfg(windows)]
mod win;

pub use clipboard::{ClipboardChange, ClipboardWatcher};
pub use error::{Error, Result};
pub use event::{InputEvent, MouseButton};
pub use focus::TextFocusWatcher;
pub use inject::{INJECTED_MARKER, Injector, Op, Sink};
pub use keys::{name_to_vk, parse_shortcut, vk_to_name};
pub use motion::accelerate;
