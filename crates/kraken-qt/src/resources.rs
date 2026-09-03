//! The compiled-in QML tree, assets and icon sources.
//!
//! `register_resources()` publishes everything under `qrc:/qml/…` and
//! `qrc:/assets/…`; `ICON_SOURCES` carries the same icons again as text, for
//! the recolouring the SVG renderer will not do for us.

include!(concat!(env!("OUT_DIR"), "/resources.rs"));
