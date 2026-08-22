//! The dock, as QML sees it.
//!
//! The arrangement itself lives in `pupo_core::dock`, which knows the rules and
//! nothing about pixels. This object is the seam: it hands QML the columns to
//! lay out, takes back the geometry QML measured, and answers the hit test that
//! turns a drag into a drop.

use std::collections::HashMap;

use pupo_core::dock::{ColumnRect, Dock, DropTarget};
use pupo_core::state;
use qmetaobject::*;
use serde_json::Value;

/// The workspace's panel order, left to right, and which of them are anchors.
///
/// History is a fixed anchor on the far left: nothing may stack with it and no
/// column may open to its left. The conversation is also un-draggable and
/// refuses stacking, but columns may open on either side of it. At most three
/// side-panel columns sit beside the conversation; later toggles stack into
/// those columns from right to left.
const ORDER: [&str; 7] = ["left", "center", "files", "browser", "diff", "git", "right"];
const FIXED: [&str; 1] = ["left"];
const NO_STACK: [&str; 1] = ["center"];
const MAX_SIDE_COLUMNS: usize = 3;

#[derive(QObject)]
pub struct DockModel {
    base: qt_base_class!(trait QObject),

    /// The columns to lay out: a list of lists of panel keys, left to right and
    /// top to bottom, holding only what is actually visible.
    columns: qt_property!(QVariantList; NOTIFY layout_changed READ get_columns),
    /// The key being dragged, or "" — QML lights that panel's grip with it.
    dragging: qt_property!(QString; NOTIFY drag_changed READ get_dragging),
    /// The drop indicator's rectangle while a drag is over a target, and
    /// whether there is one at all.
    drop_visible: qt_property!(bool; NOTIFY drag_changed READ get_drop_visible),
    drop_x: qt_property!(f64; NOTIFY drag_changed READ get_drop_x),
    drop_y: qt_property!(f64; NOTIFY drag_changed READ get_drop_y),
    drop_width: qt_property!(f64; NOTIFY drag_changed READ get_drop_width),
    drop_height: qt_property!(f64; NOTIFY drag_changed READ get_drop_height),

    layout_changed: qt_signal!(),
    drag_changed: qt_signal!(),

    set_panel_visible: qt_method!(fn(&mut self, key: QString, visible: bool)),
    is_panel_visible: qt_method!(fn(&self, key: QString) -> bool),
    /// Preferred width for a column, so a fresh one opens at a sensible size.
    preferred_width: qt_method!(fn(&self, key: QString) -> i32),
    /// The width to lay a column out at: what the user dragged it to, or the
    /// preferred width until they drag it.
    column_width: qt_method!(fn(&self, key: QString) -> i32),
    /// The narrowest the column may be dragged.
    min_width: qt_method!(fn(&self, key: QString) -> i32),
    /// Whether a divider beside this column resizes it.
    resizable: qt_method!(fn(&self, key: QString) -> bool),
    /// Take a width from a finished drag and remember it.
    set_column_width: qt_method!(fn(&mut self, key: QString, width: i32)),
    /// How far a divider actually moves when dragged, given what the columns on
    /// either side can give. The rule itself is in `pupo_core::dock`.
    resize_step: qt_method!(
        fn(&self, travel: i32, left: i32, left_min: i32, right: i32, right_min: i32) -> i32
    ),
    /// Fit the side columns into `available`, leaving the conversation its
    /// floor. Takes `[{width, min}, ...]` and answers `[width, ...]`, as JSON
    /// for the same reason `set_geometry` uses it.
    fit_columns: qt_method!(fn(&self, available: i32, columns: QString) -> QString),
    /// The panel keys that cannot fit in `available` and have to close, as a
    /// JSON array. Empty whenever everything showing still fits.
    panels_that_do_not_fit: qt_method!(fn(&self, available: i32) -> QString),
    /// Hand back the geometry QML laid the columns out at, in the same order
    /// `columns` reported them, as a JSON array of `{x, y, width, height}`.
    ///
    /// JSON rather than a list of maps because QVariant's numeric conversions
    /// are a thicket in this binding, and a hit test that silently read every
    /// coordinate as zero would be a bug with no symptom but a drop landing in
    /// the wrong column.
    set_geometry: qt_method!(fn(&mut self, rects: QString)),

    begin_drag: qt_method!(fn(&mut self, key: QString, x: f64, y: f64)),
    update_drag: qt_method!(fn(&mut self, x: f64, y: f64)),
    end_drag: qt_method!(fn(&mut self, x: f64, y: f64)),

    dock: Dock,
    rects: Vec<ColumnRect>,
    drag_key: String,
    drag_origin: Option<(f64, f64)>,
    /// A press is not a drag until the pointer has travelled far enough; below
    /// that a grab is a click on the header and moves nothing.
    drag_active: bool,
    target: Option<DropTarget>,
    /// Column widths the user has dragged to, by panel key. A key that is
    /// absent has never been dragged and falls back to its preferred width.
    widths: HashMap<String, i32>,
}

impl Default for DockModel {
    fn default() -> Self {
        Self::new()
    }
}

impl DockModel {
    pub fn new() -> Self {
        let mut dock = Dock::new(
            &ORDER,
            "center",
            &FIXED,
            &NO_STACK,
            Some(MAX_SIDE_COLUMNS),
        );
        dock.set_layout(&[&["left"], &["center"]]);
        Self {
            base: Default::default(),
            columns: Default::default(),
            dragging: Default::default(),
            drop_visible: Default::default(),
            drop_x: Default::default(),
            drop_y: Default::default(),
            drop_width: Default::default(),
            drop_height: Default::default(),
            layout_changed: Default::default(),
            drag_changed: Default::default(),
            set_panel_visible: Default::default(),
            is_panel_visible: Default::default(),
            preferred_width: Default::default(),
            column_width: Default::default(),
            min_width: Default::default(),
            resizable: Default::default(),
            set_column_width: Default::default(),
            resize_step: Default::default(),
            fit_columns: Default::default(),
            panels_that_do_not_fit: Default::default(),
            set_geometry: Default::default(),
            begin_drag: Default::default(),
            update_drag: Default::default(),
            end_drag: Default::default(),
            dock,
            rects: Vec::new(),
            drag_key: String::new(),
            drag_origin: None,
            drag_active: false,
            target: None,
            widths: load_widths(),
        }
    }

    fn get_columns(&self) -> QVariantList {
        let mut columns = QVariantList::default();
        for (_, keys) in self.dock.active_columns() {
            let mut column = QVariantList::default();
            for key in keys {
                column.push(QVariant::from(QString::from(key.as_str())));
            }
            columns.push(QVariant::from(column));
        }
        columns
    }

    fn get_dragging(&self) -> QString {
        if self.drag_active {
            self.drag_key.as_str().into()
        } else {
            "".into()
        }
    }

    fn get_drop_visible(&self) -> bool {
        self.target.is_some()
    }

    fn drop_rect(&self) -> ColumnRect {
        let empty = ColumnRect {
            x: 0.0,
            y: 0.0,
            width: 0.0,
            height: 0.0,
        };
        let Some(target) = self.target else {
            return empty;
        };
        // The target names a column by its index in the model; the geometry is
        // in the order the active columns were reported.
        let Some(slot) = self
            .dock
            .active_columns()
            .iter()
            .position(|(index, _)| *index == target.column)
        else {
            return empty;
        };
        match self.rects.get(slot) {
            Some(rect) => self.dock.zone_rect(target, *rect),
            None => empty,
        }
    }

    fn get_drop_x(&self) -> f64 {
        self.drop_rect().x
    }

    fn get_drop_y(&self) -> f64 {
        self.drop_rect().y
    }

    fn get_drop_width(&self) -> f64 {
        self.drop_rect().width
    }

    fn get_drop_height(&self) -> f64 {
        self.drop_rect().height
    }

    fn set_panel_visible(&mut self, key: QString, visible: bool) {
        let key = key.to_string();
        if self.dock.is_visible(&key) == visible {
            return;
        }
        self.dock.set_panel_visible(&key, visible);
        self.layout_changed();
    }

    fn is_panel_visible(&self, key: QString) -> bool {
        self.dock.is_visible(&key.to_string())
    }

    fn preferred_width(&self, key: QString) -> i32 {
        pupo_core::dock::preferred_width(&key.to_string())
    }

    fn column_width(&self, key: QString) -> i32 {
        let key = key.to_string();
        // History is a constant, not a preference: a stale width in the state
        // file must not bring back a 300px sidebar.
        if !pupo_core::dock::is_resizable(&key) {
            return pupo_core::dock::preferred_width(&key);
        }
        self.widths
            .get(&key)
            .copied()
            .unwrap_or_else(|| pupo_core::dock::preferred_width(&key))
            .max(pupo_core::dock::min_width(&key))
    }

    fn min_width(&self, key: QString) -> i32 {
        pupo_core::dock::min_width(&key.to_string())
    }

    fn resizable(&self, key: QString) -> bool {
        pupo_core::dock::is_resizable(&key.to_string())
    }

    fn set_column_width(&mut self, key: QString, width: i32) {
        let key = key.to_string();
        if !pupo_core::dock::is_resizable(&key) {
            return;
        }
        let width = width.max(pupo_core::dock::min_width(&key));
        if self.widths.get(&key) == Some(&width) {
            return;
        }
        self.widths.insert(key, width);
        save_widths(&self.widths);
    }

    fn resize_step(
        &self,
        travel: i32,
        left: i32,
        left_min: i32,
        right: i32,
        right_min: i32,
    ) -> i32 {
        pupo_core::dock::resize_step(
            travel,
            pupo_core::dock::Side { width: left, min: left_min },
            pupo_core::dock::Side { width: right, min: right_min },
        )
    }

    fn fit_columns(&self, available: i32, columns: QString) -> QString {
        let parsed: Vec<serde_json::Value> =
            serde_json::from_str(&columns.to_string()).unwrap_or_default();
        let sides: Vec<pupo_core::dock::Side> = parsed
            .iter()
            .map(|value| {
                let number = |key: &str| {
                    value
                        .get(key)
                        .and_then(serde_json::Value::as_i64)
                        .and_then(|n| i32::try_from(n).ok())
                        .unwrap_or(0)
                };
                pupo_core::dock::Side {
                    width: number("width"),
                    min: number("min"),
                }
            })
            .collect();
        let fitted = pupo_core::dock::fit_columns(
            available,
            pupo_core::dock::min_width("center"),
            &sides,
        );
        QString::from(serde_json::to_string(&fitted).unwrap_or_else(|_| "[]".into()))
    }

    fn panels_that_do_not_fit(&self, available: i32) -> QString {
        let columns = self.dock.active_columns();
        let history_visible = columns
            .iter()
            .any(|(_, keys)| keys.iter().any(|key| key == "left"));

        // Only the side columns are candidates; the two anchors are paid for
        // inside `side_columns_that_fit`.
        let side: Vec<&Vec<String>> = columns
            .iter()
            .filter(|(_, keys)| {
                !keys.iter().any(|key| key == "left" || key == "center")
            })
            .map(|(_, keys)| keys)
            .collect();

        // A column is sized by its first panel, the same key `column_width`
        // uses, so the two cannot disagree about what a column costs.
        let mins: Vec<i32> = side
            .iter()
            .map(|keys| {
                keys.first()
                    .map_or(0, |key| pupo_core::dock::min_width(key))
            })
            .collect();

        let fits = pupo_core::dock::side_columns_that_fit(available, history_visible, &mins);
        // Every panel in a column that does not fit, not just the one that
        // named its width: a stacked column half-closed would leave the other
        // half in a column of its own, which is a wider layout, not a narrower.
        let closing: Vec<&String> = side
            .iter()
            .skip(fits)
            .flat_map(|keys| keys.iter())
            .collect();
        QString::from(serde_json::to_string(&closing).unwrap_or_else(|_| "[]".into()))
    }

    fn set_geometry(&mut self, rects: QString) {
        let parsed: Vec<serde_json::Value> =
            serde_json::from_str(&rects.to_string()).unwrap_or_default();
        self.rects = parsed
            .iter()
            .map(|value| {
                let number = |key: &str| {
                    value.get(key).and_then(serde_json::Value::as_f64).unwrap_or(0.0)
                };
                ColumnRect {
                    x: number("x"),
                    y: number("y"),
                    width: number("width"),
                    height: number("height"),
                }
            })
            .collect();
    }

    fn begin_drag(&mut self, key: QString, x: f64, y: f64) {
        self.drag_key = key.to_string();
        self.drag_origin = Some((x, y));
        self.drag_active = false;
        self.target = None;
        self.drag_changed();
    }

    fn update_drag(&mut self, x: f64, y: f64) {
        let Some((ox, oy)) = self.drag_origin else {
            return;
        };
        if !self.drag_active {
            let travelled = (x - ox).abs() + (y - oy).abs();
            if travelled < f64::from(pupo_core::dock::DRAG_THRESHOLD) {
                return;
            }
            self.drag_active = true;
        }
        self.target = self.dock.hit_test(&self.drag_key, x, y, &self.rects);
        self.drag_changed();
    }

    fn end_drag(&mut self, x: f64, y: f64) {
        self.update_drag(x, y);
        let moved = match (self.drag_active, self.target) {
            (true, Some(target)) => {
                let key = std::mem::take(&mut self.drag_key);
                let moved = self.dock.apply_drop(&key, target);
                self.drag_key = key;
                moved
            }
            _ => false,
        };
        self.drag_key.clear();
        self.drag_origin = None;
        self.drag_active = false;
        self.target = None;
        self.drag_changed();
        if moved {
            self.layout_changed();
        }
    }

}

/// Column widths live beside the rest of the workspace state, under one key.
///
/// They are global rather than per-workspace: how wide someone wants the git
/// log is a fact about the panel, not about the project open in front of it.
const WIDTHS_KEY: &str = "panel_widths";

fn load_widths() -> HashMap<String, i32> {
    state::object(WIDTHS_KEY)
        .into_iter()
        .filter_map(|(key, value)| {
            let width = value.as_i64()?;
            i32::try_from(width).ok().map(|width| (key, width))
        })
        .collect()
}

fn save_widths(widths: &HashMap<String, i32>) {
    let map = widths
        .iter()
        .map(|(key, width)| (key.clone(), Value::from(*width)))
        .collect();
    state::set(WIDTHS_KEY, Value::Object(map));
}
