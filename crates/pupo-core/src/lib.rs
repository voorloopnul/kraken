//! Pupo's application core.
//!
//! Everything the app knows how to *do*, with no dependency on Qt: the theme
//! and type scales, persistent state, the Pi agent RPC client and its on-disk
//! configuration, remote (SSH) workspaces, git surfaces, the chat pipeline, the
//! terminal engine. The Qt layer in `pupo-qt` is a view over
//! this crate, which keeps the interesting parts unit-testable without a
//! display.

pub mod browser;
pub mod chat;
pub mod debug;
pub mod diff;
pub mod dock;
pub mod external;
pub mod files;
pub mod git;
pub mod pi;
pub mod remote;
pub mod settings;
pub mod state;
pub mod terminal;
pub mod theme;
pub mod typography;
pub mod util;
pub mod workspace;
