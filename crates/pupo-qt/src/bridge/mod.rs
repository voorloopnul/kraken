//! The QObjects QML talks to.
//!
//! Each module here is a thin view over `pupo-core`: it converts between Qt
//! types and Rust ones and turns core callbacks into Qt signals, and holds no
//! logic of its own worth testing without a display. Anything that would be
//! worth a test belongs in the core crate instead — that separation is why the
//! interesting half of this app is testable without a display at all.

pub mod app;
pub mod browser;
pub mod diff;
pub mod dock;
pub mod git;
pub mod history;
pub mod remotes;
pub mod session;
pub mod settings;
pub mod terminal;
pub mod theme;
