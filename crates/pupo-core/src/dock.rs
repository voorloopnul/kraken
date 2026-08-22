//! The workspace's panel layout, and the rules a drag between panels obeys.
//!
//! The dock arranges content panels into columns laid out left to right. Each
//! column holds one or two panels, so the workspace can stack (for example) the
//! terminal above the git history in a single column. Every panel carries a
//! slim header that doubles as a drag handle: grab it and drop the panel beside
//! a column (to open a new column) or onto the top or bottom half of a column
//! (to stack it there, up to the two-panel limit). A workspace also caps the
//! number of side-panel columns; once that cap is reached, a newly shown panel
//! fills the columns from right to left instead of making the workspace wider.
//!
//! Only the *arrangement* lives here. The view reads this model and reparents
//! the real panels, so a panel's own state — terminals, a browser, transcripts —
//! survives a move untouched, and every rule below is testable without a
//! display, which is what the widget version could never manage.

use std::collections::HashSet;

/// Preferred column widths per panel key, used to size a column the first time
/// it opens and whenever the user has not set a width of their own.
pub fn preferred_width(key: &str) -> i32 {
    match key {
        "left" => HISTORY_WIDTH,
        "center" => 700,
        "browser" => 480,
        "files" => 300,
        "diff" => 380,
        "git" => 360,
        "right" => 460,
        _ => 400,
    }
}

/// History's width, which is not a preference but a constant: the panel is a
/// list of session titles and a button, it has nothing to do with more room,
/// and it is the one column a drag cannot resize.
pub const HISTORY_WIDTH: i32 = 200;

/// The narrowest a column may be dragged.
///
/// The conversation's floor is the one that matters in practice — it is the
/// column that absorbs whatever the others leave, so without a floor a wide
/// enough side panel would squeeze the composer into nothing.
pub fn min_width(key: &str) -> i32 {
    match key {
        "left" => HISTORY_WIDTH,
        "center" => 350,
        _ => 260,
    }
}

/// Whether a divider beside this column may be dragged to resize it.
///
/// History is fixed. The conversation is not resized directly either: it takes
/// the slack the other columns leave, so it is resized by moving them.
pub fn is_resizable(key: &str) -> bool {
    key != "left" && key != "center"
}

/// One side of a divider, as far as a drag is concerned: how wide it is now and
/// how narrow it may get.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Side {
    pub width: i32,
    pub min: i32,
}

impl Side {
    /// How much this side can give up before it hits its floor.
    fn give(&self) -> i32 {
        (self.width - self.min).max(0)
    }
}

/// How far a divider actually moves when it is dragged `travel` pixels.
///
/// A divider moves the boundary between the two columns it sits between, and
/// the rule is the same whichever they are: to move right, the side on the
/// right must give up that much width; to move left, the side on the left must.
/// Whichever side is the conversation gives and takes through the stretch
/// column instead of a width of its own, because that is the column with no
/// stored width to change.
///
/// Getting this wrong is not obvious on screen — a divider that will not widen
/// looks like a divider that is simply stuck — so the arithmetic lives here,
/// where it can be checked without a display.
pub fn resize_step(travel: i32, left: Side, right: Side) -> i32 {
    travel.clamp(-left.give(), right.give())
}

/// Fit the side columns into the width there is, leaving the conversation at
/// least `stretch_min`.
///
/// When they already fit, everyone keeps what they asked for and the
/// conversation takes the rest. When they do not, the excess is shared out in
/// proportion — but a column that would be pushed under its floor stops there
/// and the rest make up its share between them, which is why this is a loop and
/// not one multiplication.
///
/// Scaling every column by one factor is the obvious version and it is wrong:
/// the columns pinned at their floor keep more than their proportional share,
/// so the total lands over budget by however much the floors held back, and the
/// rightmost panel hangs off the edge of the window.
pub fn fit_columns(available: i32, stretch_min: i32, columns: &[Side]) -> Vec<i32> {
    let budget = (available - stretch_min).max(0);
    let wanted: i32 = columns.iter().map(|side| side.width).sum();
    if wanted <= budget {
        return columns.iter().map(|side| side.width).collect();
    }

    // `None` while a column is still sharing the shortfall; `Some` once it has
    // hit its floor and can give no more.
    let mut pinned: Vec<Option<i32>> = vec![None; columns.len()];
    loop {
        let pinned_total: i32 = pinned.iter().flatten().sum();
        let free: Vec<usize> = (0..columns.len()).filter(|i| pinned[*i].is_none()).collect();
        if free.is_empty() {
            break;
        }
        let free_wanted: i32 = free.iter().map(|i| columns[*i].width).sum();
        let free_budget = budget - pinned_total;
        if free_wanted <= free_budget || free_wanted <= 0 {
            break;
        }

        let scale = f64::from(free_budget.max(0)) / f64::from(free_wanted);
        let mut newly_pinned = false;
        for index in &free {
            let scaled = (f64::from(columns[*index].width) * scale).round() as i32;
            if scaled < columns[*index].min {
                pinned[*index] = Some(columns[*index].min);
                newly_pinned = true;
            }
        }
        if !newly_pinned {
            return (0..columns.len())
                .map(|i| match pinned[i] {
                    Some(width) => width,
                    None => (f64::from(columns[i].width) * scale).round() as i32,
                })
                .collect();
        }
    }

    // Everything that could give has given; whatever is left keeps its width.
    (0..columns.len())
        .map(|i| pinned[i].unwrap_or(columns[i].width))
        .collect()
}

/// How many side columns fit, counted left to right, once the anchors have
/// taken theirs.
///
/// History is a fixed width and the conversation has a floor, so neither can
/// give: past a certain narrowness the only thing left to do is show fewer
/// panels. Without this the columns that do not fit are still laid out — they
/// simply run off the right edge of the window, where they cannot be reached or
/// even seen.
///
/// Counted from the left so the panels nearest the conversation are the ones
/// kept, and the furthest are the first to go.
pub fn side_columns_that_fit(available: i32, history_visible: bool, mins: &[i32]) -> usize {
    let anchors = min_width("center") + if history_visible { HISTORY_WIDTH } else { 0 };
    let mut budget = available - anchors;
    let mut fit = 0;
    for min in mins {
        if budget < *min {
            break;
        }
        budget -= *min;
        fit += 1;
    }
    fit
}

/// How far into a column counts as its side band (a drop there opens a new
/// column) versus its middle (a drop there stacks into the column).
pub const EDGE_BAND: f64 = 0.28;
/// Pixels the header must travel before a click turns into a drag.
pub const DRAG_THRESHOLD: i32 = 6;
/// A column never holds more than this many panels.
pub const MAX_STACK: usize = 2;

/// Where a drop would put the dragged panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropMode {
    NewBefore,
    NewAfter,
    StackTop,
    StackBottom,
}

/// A resolved drop: a mode plus the column it is relative to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DropTarget {
    pub mode: DropMode,
    pub column: usize,
}

/// A column's on-screen geometry, as the view measured it. The model does no
/// layout of its own — it only needs to know where the columns ended up in
/// order to answer a hit test.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColumnRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl ColumnRect {
    fn right(&self) -> f64 {
        self.x + self.width
    }
}

#[derive(Debug, Clone)]
pub struct Dock {
    /// The canonical left-to-right key order. A panel shown from hidden slots
    /// into a fresh column at the position this order implies.
    order: Vec<String>,
    /// The panel whose column absorbs spare width (the conversation).
    stretch_key: String,
    /// Hard anchors: never stacked with, and nothing may open a column to
    /// their left.
    fixed: HashSet<String>,
    /// Softer: nothing may stack with them either, but columns can still open
    /// on either side. Every fixed panel is also one of these.
    no_stack: HashSet<String>,
    /// Side panels are the draggable ones; fixed workspace anchors such as
    /// History and the conversation do not count toward this limit.
    max_side_columns: Option<usize>,
    /// Placement, left to right, each column top to bottom. A hidden panel
    /// stays in its column so a later show can restore it there.
    columns: Vec<Vec<String>>,
    shown: HashSet<String>,
}

impl Dock {
    pub fn new(
        order: &[&str],
        stretch_key: &str,
        fixed_keys: &[&str],
        no_stack_keys: &[&str],
        max_side_columns: Option<usize>,
    ) -> Self {
        let fixed: HashSet<String> = fixed_keys.iter().map(|k| (*k).to_string()).collect();
        let mut no_stack: HashSet<String> =
            no_stack_keys.iter().map(|k| (*k).to_string()).collect();
        no_stack.extend(fixed.iter().cloned());
        Self {
            order: order.iter().map(|k| (*k).to_string()).collect(),
            stretch_key: stretch_key.to_string(),
            fixed,
            no_stack,
            max_side_columns,
            columns: Vec::new(),
            shown: HashSet::new(),
        }
    }

    /// The layout a workspace opens with: History and the conversation, each in
    /// a column of its own.
    pub fn set_layout(&mut self, columns: &[&[&str]]) {
        self.columns = columns
            .iter()
            .map(|keys| keys.iter().map(|k| (*k).to_string()).collect())
            .collect();
        self.shown = columns
            .iter()
            .flat_map(|keys| keys.iter().map(|k| (*k).to_string()))
            .collect();
    }

    pub fn columns(&self) -> &[Vec<String>] {
        &self.columns
    }

    /// The columns with something visible in them, as `(index, keys)`.
    pub fn active_columns(&self) -> Vec<(usize, Vec<String>)> {
        self.columns
            .iter()
            .enumerate()
            .filter(|(_, keys)| keys.iter().any(|k| self.shown.contains(k)))
            .map(|(i, keys)| {
                (
                    i,
                    keys.iter()
                        .filter(|k| self.shown.contains(*k))
                        .cloned()
                        .collect(),
                )
            })
            .collect()
    }

    pub fn is_visible(&self, key: &str) -> bool {
        self.shown.contains(key)
    }

    fn draggable(&self, key: &str) -> bool {
        !self.fixed.contains(key) && key != self.stretch_key
    }

    fn column_of(&self, key: &str) -> Option<usize> {
        self.columns.iter().position(|keys| keys.iter().any(|k| k == key))
    }

    fn column_has_shown(&self, index: usize) -> bool {
        self.columns[index].iter().any(|k| self.shown.contains(k))
    }

    /// Active columns belonging to draggable, right-side panels.
    fn active_side_columns(&self) -> Vec<usize> {
        self.active_columns()
            .into_iter()
            .filter(|(index, _)| {
                self.columns[*index].iter().any(|k| self.draggable(k))
            })
            .map(|(index, _)| index)
            .collect()
    }

    fn side_column_limit_reached(&self) -> bool {
        match self.max_side_columns {
            Some(limit) => self.active_side_columns().len() >= limit,
            None => false,
        }
    }

    /// Space for the next toggled panel, searched from right to left.
    fn auto_stack_column(&self, source: Option<usize>) -> Option<usize> {
        for index in self.active_side_columns().into_iter().rev() {
            if Some(index) == source {
                continue;
            }
            let keys = &self.columns[index];
            if keys.iter().any(|k| self.no_stack.contains(k)) {
                continue;
            }
            if keys.len() < MAX_STACK {
                return Some(index);
            }
        }
        None
    }

    /// Whether moving `key` into a new column would stay under the cap.
    ///
    /// Moving a lone visible panel only relocates its column and is allowed;
    /// pulling one panel out of a visible stack would add a column.
    fn can_open_side_column(&self, key: Option<&str>) -> bool {
        let Some(key) = key else { return true };
        if !self.draggable(key) || self.max_side_columns.is_none() {
            return true;
        }
        let active = self.active_side_columns();
        let source_will_close = self
            .column_of(key)
            .filter(|index| active.contains(index))
            .map(|index| {
                !self.columns[index]
                    .iter()
                    .any(|other| other != key && self.shown.contains(other))
            })
            .unwrap_or(false);
        let projected = active.len() + 1 - usize::from(source_will_close);
        projected <= self.max_side_columns.unwrap_or(usize::MAX)
    }

    /// Column index for a fresh column holding `key`, placed before the first
    /// active column that ranks after it in the canonical order.
    fn insertion_index(&self, key: &str) -> usize {
        let Some(rank) = self.order.iter().position(|k| k == key) else {
            return self.columns.len();
        };
        for (index, keys) in self.active_columns() {
            let best = keys
                .iter()
                .filter_map(|k| self.order.iter().position(|o| o == k))
                .min();
            if let Some(best) = best {
                if best > rank {
                    return index;
                }
            }
        }
        self.columns.len()
    }

    fn detach(&mut self, key: &str) -> Option<usize> {
        let index = self.column_of(key)?;
        self.columns[index].retain(|k| k != key);
        Some(index)
    }

    /// Drop any column left holding no panel at all. A column that still holds
    /// a hidden panel is kept, so showing that panel again restores it in place.
    fn prune(&mut self) {
        self.columns.retain(|keys| !keys.is_empty());
    }

    /// Reveal a panel.
    ///
    /// It reappears in whatever column it last lived in; a panel that has never
    /// been placed gets a fresh column positioned to match the canonical order.
    /// Once the side-column limit is full, a panel whose own column would add
    /// another one stacks into the first available column, working right to
    /// left.
    pub fn show_panel(&mut self, key: &str) {
        if self.shown.contains(key) {
            return;
        }
        let source = self.column_of(key);
        let source_is_active = source.map(|i| self.column_has_shown(i)).unwrap_or(false);
        let stack = if self.draggable(key)
            && !source_is_active
            && self.side_column_limit_reached()
        {
            self.auto_stack_column(source)
        } else {
            None
        };

        if let Some(stack) = stack {
            self.detach(key);
            self.columns[stack].push(key.to_string());
        } else if source.is_none() {
            let at = self.insertion_index(key);
            self.columns.insert(at, vec![key.to_string()]);
        }
        self.shown.insert(key.to_string());
        self.prune();
    }

    /// Hide a panel in place — it stays in its column so a later show restores
    /// it there.
    pub fn hide_panel(&mut self, key: &str) {
        self.shown.remove(key);
    }

    pub fn set_panel_visible(&mut self, key: &str, visible: bool) {
        if visible {
            self.show_panel(key);
        } else {
            self.hide_panel(key);
        }
    }

    /// Map a cursor position over the laid-out columns to a drop target.
    ///
    /// `rects` are the *active* columns' geometries in the same order
    /// [`active_columns`] returns them. `None` means the drop would be a no-op,
    /// would exceed two panels in a column, or is not allowed at all — and the
    /// drag can be cancelled by releasing there.
    pub fn hit_test(
        &self,
        dragged: &str,
        x: f64,
        y: f64,
        rects: &[ColumnRect],
    ) -> Option<DropTarget> {
        let active = self.active_columns();
        if active.is_empty() || rects.len() != active.len() {
            return None;
        }
        let slot = active
            .iter()
            .position(|_| false)
            .or_else(|| rects.iter().position(|rect| x < rect.right()))
            .unwrap_or(rects.len() - 1);
        let rect = rects[slot];
        let index = active[slot].0;
        let keys = &self.columns[index];

        // A fixed panel hard-anchors its column: no new column opens to its
        // left. A no-stack panel (which every fixed one also is) only refuses
        // to be stacked with.
        let fixed_col = keys.iter().any(|k| self.fixed.contains(k));
        let nostack_col = keys.iter().any(|k| self.no_stack.contains(k));

        let rel_x = (x - rect.x) / rect.width.max(1.0);
        if rel_x < EDGE_BAND {
            if fixed_col || !self.can_open_side_column(Some(dragged)) {
                return None;
            }
            return self.collapse_noop(DropMode::NewBefore, index, dragged);
        }
        if rel_x > 1.0 - EDGE_BAND {
            if !self.can_open_side_column(Some(dragged)) {
                return None;
            }
            return self.collapse_noop(DropMode::NewAfter, index, dragged);
        }

        // Middle band: stack into this column, unless it refuses stacking or
        // already holds two panels other than the one being dragged. The limit
        // counts every panel in the column — a hidden co-panel still occupies it.
        if nostack_col || (keys.len() >= MAX_STACK && !keys.iter().any(|k| k == dragged)) {
            return None;
        }
        let rel_y = (y - rect.y) / rect.height.max(1.0);
        Some(DropTarget {
            mode: if rel_y < 0.5 {
                DropMode::StackTop
            } else {
                DropMode::StackBottom
            },
            column: index,
        })
    }

    /// A "new column beside" target that would just put the dragged panel back
    /// where it already sits alone changes nothing, so it is not a target.
    fn collapse_noop(&self, mode: DropMode, index: usize, dragged: &str) -> Option<DropTarget> {
        let shown: Vec<&String> = self.columns[index]
            .iter()
            .filter(|k| self.shown.contains(*k))
            .collect();
        if shown.len() == 1 && shown[0] == dragged {
            return None;
        }
        Some(DropTarget { mode, column: index })
    }

    /// The rectangle the drop indicator paints, given the target column's own.
    pub fn zone_rect(&self, target: DropTarget, rect: ColumnRect) -> ColumnRect {
        match target.mode {
            DropMode::NewBefore => ColumnRect {
                width: rect.width * 0.4,
                ..rect
            },
            DropMode::NewAfter => ColumnRect {
                x: rect.right() - rect.width * 0.4,
                width: rect.width * 0.4,
                ..rect
            },
            DropMode::StackTop => ColumnRect {
                height: rect.height / 2.0,
                ..rect
            },
            DropMode::StackBottom => ColumnRect {
                y: rect.y + rect.height / 2.0,
                height: rect.height / 2.0,
                ..rect
            },
        }
    }

    /// Move `key` to where `target` says. Returns whether anything moved.
    pub fn apply_drop(&mut self, key: &str, target: DropTarget) -> bool {
        if matches!(target.mode, DropMode::NewBefore | DropMode::NewAfter)
            && !self.can_open_side_column(Some(key))
        {
            return false;
        }
        let source = self.column_of(key);
        match target.mode {
            DropMode::StackTop | DropMode::StackBottom => {
                if source == Some(target.column) && self.columns[target.column].len() <= 1 {
                    return false; // already the lone panel here; nothing to reorder
                }
                self.detach(key);
                let at = if target.mode == DropMode::StackTop {
                    0
                } else {
                    self.columns[target.column].len()
                };
                self.columns[target.column].insert(at, key.to_string());
            }
            DropMode::NewBefore | DropMode::NewAfter => {
                self.detach(key);
                let at = if target.mode == DropMode::NewBefore {
                    target.column
                } else {
                    target.column + 1
                };
                self.columns.insert(at, vec![key.to_string()]);
            }
        }
        self.shown.insert(key.to_string());
        self.prune();
        true
    }

    /// Column widths after a structural change.
    ///
    /// Each surviving column keeps the width it had in `previous`; a column
    /// that is newly shown takes its preferred width; the stretch column (the
    /// conversation) absorbs the slack so the row still fills the dock. `gap`
    /// is the width of one divider — budget for them here, or every reflow
    /// over-allocates and the columns meant to be preserved get shaved down.
    pub fn reflow(&self, available: f64, gap: f64, previous: &[(usize, f64)]) -> Vec<(usize, f64)> {
        let active = self.active_columns();
        if active.is_empty() {
            return Vec::new();
        }
        let stretch = active
            .iter()
            .position(|(index, _)| self.columns[*index].contains(&self.stretch_key));

        let mut widths: Vec<(usize, f64)> = Vec::with_capacity(active.len());
        let mut fixed_total = 0.0;
        for (slot, (index, _)) in active.iter().enumerate() {
            if Some(slot) == stretch {
                widths.push((*index, 0.0));
                continue;
            }
            let kept = previous
                .iter()
                .find(|(i, _)| i == index)
                .map(|(_, w)| *w)
                .unwrap_or(0.0);
            let width = if kept > 0.0 {
                kept
            } else {
                f64::from(
                    self.columns[*index]
                        .iter()
                        .map(|k| preferred_width(k))
                        .max()
                        .unwrap_or(400),
                )
            };
            fixed_total += width;
            widths.push((*index, width));
        }
        if let Some(slot) = stretch {
            let usable = available - gap * (active.len().saturating_sub(1)) as f64;
            let index = widths[slot].0;
            widths[slot].1 = if usable > 0.0 {
                (usable - fixed_total).max(1.0)
            } else {
                f64::from(
                    self.columns[index]
                        .iter()
                        .map(|k| preferred_width(k))
                        .max()
                        .unwrap_or(400),
                )
            };
        }
        widths
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The workspace's own dock: History anchored far left, the conversation
    /// next to it, and at most three side columns beside them.
    fn workspace_dock() -> Dock {
        let mut dock = Dock::new(
            &["left", "center", "browser", "diff", "git", "right"],
            "center",
            &["left"],
            &["center"],
            Some(3),
        );
        dock.set_layout(&[&["left"], &["center"]]);
        dock
    }

    fn keys(dock: &Dock) -> Vec<Vec<String>> {
        dock.active_columns().into_iter().map(|(_, k)| k).collect()
    }

    #[test]
    fn a_workspace_opens_on_history_and_the_conversation() {
        let dock = workspace_dock();
        assert_eq!(keys(&dock), vec![vec!["left"], vec!["center"]]);
        assert!(dock.is_visible("left"));
        assert!(!dock.is_visible("right"));
    }

    #[test]
    fn a_shown_panel_lands_where_the_canonical_order_puts_it() {
        let mut dock = workspace_dock();
        dock.show_panel("right");
        dock.show_panel("diff");
        // diff ranks before right, so it opens to the left of it.
        assert_eq!(
            keys(&dock),
            vec![vec!["left"], vec!["center"], vec!["diff"], vec!["right"]]
        );
    }

    #[test]
    fn hiding_a_panel_keeps_its_place_for_next_time() {
        let mut dock = workspace_dock();
        dock.show_panel("git");
        dock.show_panel("right");
        dock.hide_panel("git");
        assert_eq!(keys(&dock), vec![vec!["left"], vec!["center"], vec!["right"]]);
        dock.show_panel("git");
        // Back in its own column, left of the terminal, not stacked into it.
        assert_eq!(
            keys(&dock),
            vec![vec!["left"], vec!["center"], vec!["git"], vec!["right"]]
        );
    }

    #[test]
    fn past_the_side_column_cap_panels_stack_from_the_right() {
        let mut dock = workspace_dock();
        for key in ["browser", "diff", "git"] {
            dock.show_panel(key);
        }
        assert_eq!(dock.active_columns().len(), 5); // left, center, + three sides
        dock.show_panel("right");
        // No fourth side column: the terminal stacks into the rightmost one
        // that will take it.
        assert_eq!(
            keys(&dock),
            vec![
                vec!["left"],
                vec!["center"],
                vec!["browser"],
                vec!["diff"],
                vec!["git", "right"],
            ]
        );
    }

    #[test]
    fn nothing_stacks_with_history_or_the_conversation() {
        let mut dock = workspace_dock();
        dock.show_panel("right");
        let rects = column_rects(&dock);
        // The middle of History's column and of the conversation's are both
        // refused.
        for slot in [0usize, 1] {
            let rect = rects[slot];
            let middle = rect.x + rect.width / 2.0;
            assert_eq!(dock.hit_test("right", middle, rect.y + 10.0, &rects), None);
        }
    }

    #[test]
    fn no_column_opens_to_the_left_of_history() {
        let mut dock = workspace_dock();
        dock.show_panel("right");
        let rects = column_rects(&dock);
        let rect = rects[0];
        assert_eq!(
            dock.hit_test("right", rect.x + rect.width * 0.1, rect.y + 10.0, &rects),
            None
        );
    }

    /// Columns laid out 300px wide each, 800 tall, in the order the dock
    /// reports them.
    fn column_rects(dock: &Dock) -> Vec<ColumnRect> {
        dock.active_columns()
            .iter()
            .enumerate()
            .map(|(slot, _)| ColumnRect {
                x: slot as f64 * 300.0,
                y: 0.0,
                width: 300.0,
                height: 800.0,
            })
            .collect()
    }

    #[test]
    fn the_edge_bands_open_a_column_and_the_middle_stacks() {
        let mut dock = workspace_dock();
        dock.show_panel("git");
        dock.show_panel("right");
        let rects = column_rects(&dock);
        let git = rects[2];
        // Dragging the terminal onto the git column: left band, right band,
        // top half, bottom half.
        let at = |rel_x: f64, rel_y: f64| {
            dock.hit_test(
                "right",
                git.x + git.width * rel_x,
                git.y + git.height * rel_y,
                &rects,
            )
            .map(|t| t.mode)
        };
        assert_eq!(at(0.05, 0.5), Some(DropMode::NewBefore));
        assert_eq!(at(0.95, 0.5), Some(DropMode::NewAfter));
        assert_eq!(at(0.5, 0.2), Some(DropMode::StackTop));
        assert_eq!(at(0.5, 0.8), Some(DropMode::StackBottom));
    }

    #[test]
    fn dropping_a_lone_panel_beside_its_own_column_is_not_a_target() {
        let mut dock = workspace_dock();
        dock.show_panel("right");
        let rects = column_rects(&dock);
        let own = rects[2];
        assert_eq!(
            dock.hit_test("right", own.x + own.width * 0.05, own.y + 10.0, &rects),
            None
        );
        assert_eq!(
            dock.hit_test("right", own.x + own.width * 0.95, own.y + 10.0, &rects),
            None
        );
    }

    #[test]
    fn a_column_will_not_take_a_third_panel() {
        let mut dock = workspace_dock();
        dock.show_panel("git");
        dock.show_panel("diff");
        dock.show_panel("right");
        // Stack diff onto git, then try to add the terminal to the pair.
        let rects = column_rects(&dock);
        let git_slot = dock
            .active_columns()
            .iter()
            .position(|(_, keys)| keys.iter().any(|k| k == "git"))
            .unwrap();
        let git = rects[git_slot];
        let target = dock
            .hit_test("diff", git.x + git.width / 2.0, git.y + 10.0, &rects)
            .expect("stacking onto git is allowed");
        assert!(dock.apply_drop("diff", target));
        let rects = column_rects(&dock);
        let stacked = dock
            .active_columns()
            .iter()
            .position(|(_, keys)| keys.len() == 2)
            .unwrap();
        let rect = rects[stacked];
        assert_eq!(
            dock.hit_test("right", rect.x + rect.width / 2.0, rect.y + 10.0, &rects),
            None
        );
    }

    #[test]
    fn stacking_puts_the_panel_where_the_half_says() {
        let mut dock = workspace_dock();
        dock.show_panel("git");
        dock.show_panel("right");
        let git_index = dock
            .active_columns()
            .iter()
            .find(|(_, keys)| keys.iter().any(|k| k == "git"))
            .map(|(i, _)| *i)
            .unwrap();
        dock.apply_drop(
            "right",
            DropTarget { mode: DropMode::StackTop, column: git_index },
        );
        assert_eq!(
            keys(&dock),
            vec![vec!["left"], vec!["center"], vec!["right", "git"]]
        );
    }

    #[test]
    fn a_column_emptied_by_a_drop_disappears() {
        let mut dock = workspace_dock();
        dock.show_panel("git");
        dock.show_panel("right");
        assert_eq!(dock.active_columns().len(), 4);
        let git_index = dock
            .active_columns()
            .iter()
            .find(|(_, keys)| keys.iter().any(|k| k == "git"))
            .map(|(i, _)| *i)
            .unwrap();
        dock.apply_drop(
            "right",
            DropTarget { mode: DropMode::StackBottom, column: git_index },
        );
        assert_eq!(dock.active_columns().len(), 3);
        assert_eq!(keys(&dock).last().unwrap(), &vec!["git", "right"]);
    }

    #[test]
    fn the_zone_is_the_half_or_the_edge_it_promises() {
        let dock = workspace_dock();
        let rect = ColumnRect { x: 100.0, y: 0.0, width: 300.0, height: 800.0 };
        let zone = |mode| dock.zone_rect(DropTarget { mode, column: 0 }, rect);
        assert_eq!(zone(DropMode::NewBefore).x, 100.0);
        assert_eq!(zone(DropMode::NewBefore).width, 120.0);
        assert_eq!(zone(DropMode::NewAfter).x, 280.0);
        assert_eq!(zone(DropMode::StackTop).height, 400.0);
        assert_eq!(zone(DropMode::StackBottom).y, 400.0);
    }

    #[test]
    fn the_conversation_absorbs_the_slack_and_the_others_keep_their_widths() {
        let mut dock = workspace_dock();
        dock.show_panel("right");
        let widths = dock.reflow(1360.0, 1.0, &[]);
        let by_key = |key: &str| {
            let index = dock.column_of(key).unwrap();
            widths.iter().find(|(i, _)| *i == index).unwrap().1
        };
        assert_eq!(by_key("left"), f64::from(HISTORY_WIDTH));
        assert_eq!(by_key("right"), 460.0);
        // 1360 - two dividers - 200 - 460
        assert_eq!(by_key("center"), 698.0);
        assert_eq!(widths.iter().map(|(_, w)| w).sum::<f64>() + 2.0, 1360.0);
    }

    #[test]
    fn history_is_a_fixed_width_and_nothing_may_drag_it() {
        assert_eq!(preferred_width("left"), HISTORY_WIDTH);
        assert_eq!(min_width("left"), HISTORY_WIDTH);
        assert!(!is_resizable("left"));
    }

    #[test]
    fn the_conversation_has_a_floor_but_is_resized_only_through_its_neighbours() {
        assert_eq!(min_width("center"), 350);
        // It takes the slack the others leave, so a divider never targets it.
        assert!(!is_resizable("center"));
    }

    #[test]
    fn every_side_panel_can_be_dragged_and_stops_at_a_floor() {
        for key in ["files", "browser", "diff", "git", "right"] {
            assert!(is_resizable(key), "{key} should be resizable");
            assert!(
                min_width(key) <= preferred_width(key),
                "{key} opens narrower than it may be dragged"
            );
        }
    }

    #[test]
    fn a_divider_between_two_side_panels_takes_width_from_its_neighbour() {
        // The case that was broken: the conversation is pinned at its floor, so
        // it has nothing to give — but Git beside it does, and widening Changes
        // must come out of Git rather than being refused.
        let changes = Side { width: 300, min: 240 };
        let git = Side { width: 400, min: 240 };
        assert_eq!(resize_step(80, changes, git), 80);
        // Only as far as Git's own floor, though.
        assert_eq!(resize_step(500, changes, git), 160);
    }

    #[test]
    fn a_divider_stops_where_either_side_reaches_its_floor() {
        let left = Side { width: 260, min: 240 };
        let right = Side { width: 250, min: 240 };
        assert_eq!(resize_step(100, left, right), 10, "right can give only 10");
        assert_eq!(resize_step(-100, left, right), -20, "left can give only 20");
    }

    #[test]
    fn a_side_at_its_floor_gives_nothing_but_can_still_receive() {
        let squeezed = Side { width: 240, min: 240 };
        let roomy = Side { width: 500, min: 240 };
        // Dragging into the squeezed side is refused...
        assert_eq!(resize_step(-60, squeezed, roomy), 0);
        // ...but it may still be widened at the other's expense.
        assert_eq!(resize_step(60, squeezed, roomy), 60);
    }

    #[test]
    fn a_conversation_at_its_floor_refuses_to_shrink_further() {
        // The conversation on the right of the divider, already at 400.
        let panel = Side { width: 480, min: 240 };
        let conversation = Side { width: 400, min: 400 };
        assert_eq!(resize_step(120, panel, conversation), 0, "nothing left to take");
        // Giving width back to it always works.
        assert_eq!(resize_step(-120, panel, conversation), -120);
    }

    #[test]
    fn columns_that_already_fit_are_left_alone() {
        let columns = [Side { width: 200, min: 200 }, Side { width: 300, min: 240 }];
        assert_eq!(fit_columns(1360, 400, &columns), vec![200, 300]);
    }

    /// The bug this replaced: scaling every column by one factor lets the ones
    /// pinned at their floor keep more than their share, and the total lands
    /// over budget — on screen, the last panel hangs off the right of the
    /// window. Whatever the floors hold back has to come off the columns that
    /// still have room.
    #[test]
    fn columns_pinned_at_their_floor_do_not_push_the_total_over_budget() {
        let columns = [
            Side { width: 200, min: 200 }, // history: nothing to give
            Side { width: 363, min: 240 },
            Side { width: 408, min: 240 },
            Side { width: 727, min: 240 },
        ];
        let fitted = fit_columns(1360, 400, &columns);
        assert_eq!(fitted.iter().sum::<i32>(), 960, "must fit 1360 less the 400 floor");
        for (got, side) in fitted.iter().zip(columns.iter()) {
            assert!(*got >= side.min, "{got} is under the {} floor", side.min);
        }
    }

    #[test]
    fn the_widest_column_gives_up_the_most() {
        let columns = [
            Side { width: 300, min: 240 },
            Side { width: 900, min: 240 },
        ];
        let fitted = fit_columns(1360, 400, &columns);
        assert_eq!(fitted.iter().sum::<i32>(), 960);
        assert!(
            (columns[1].width - fitted[1]) > (columns[0].width - fitted[0]),
            "the wide one should absorb more of the shortfall: {fitted:?}"
        );
    }

    #[test]
    fn columns_never_shrink_past_their_floor_even_with_no_room_left() {
        // Far more panels than will fit: every one lands on its floor rather
        // than being scaled into nothing.
        let columns = [Side { width: 400, min: 240 }; 5];
        let fitted = fit_columns(800, 400, &columns);
        assert!(fitted.iter().all(|width| *width == 240), "{fitted:?}");
    }

    #[test]
    fn every_side_panel_fits_in_a_wide_window() {
        let mins = [240, 240, 240];
        assert_eq!(side_columns_that_fit(1440, true, &mins), 3);
    }

    /// 900 is the window's own minimum. History takes 200 and the conversation
    /// 400, which leaves 300 — room for one side panel at its floor, not three.
    /// The other two have to close rather than be laid out off the screen.
    #[test]
    fn the_narrowest_window_keeps_one_side_panel() {
        let mins = [240, 240, 240];
        assert_eq!(side_columns_that_fit(900, true, &mins), 1);
    }

    #[test]
    fn hiding_history_pays_for_another_side_panel() {
        let mins = [240, 240, 240];
        assert_eq!(side_columns_that_fit(900, false, &mins), 2);
    }

    #[test]
    fn a_window_with_no_room_at_all_keeps_no_side_panels() {
        let mins = [240, 240, 240];
        // Below the anchors themselves nothing beside them can be shown.
        assert_eq!(side_columns_that_fit(620, true, &mins), 0);
    }

    #[test]
    fn a_wider_column_is_counted_at_its_own_floor() {
        // A column whose first panel has a larger minimum takes more of the
        // budget, so fewer fit beside it.
        let mins = [400, 240];
        assert_eq!(side_columns_that_fit(1000, true, &mins), 1);
        assert_eq!(side_columns_that_fit(1240, true, &mins), 2);
    }

    #[test]
    fn a_column_that_was_already_on_screen_keeps_the_width_it_had() {
        let mut dock = workspace_dock();
        dock.show_panel("right");
        let left = dock.column_of("left").unwrap();
        let widths = dock.reflow(1360.0, 1.0, &[(left, 220.0)]);
        assert_eq!(widths.iter().find(|(i, _)| *i == left).unwrap().1, 220.0);
    }

    #[test]
    fn without_a_stretch_column_nothing_is_over_allocated() {
        let mut dock = Dock::new(&["a", "b"], "none", &[], &[], None);
        dock.set_layout(&[&["a"], &["b"]]);
        let widths = dock.reflow(1000.0, 1.0, &[]);
        // Both fall back to the default preferred width; no column grows to
        // fill, because none of them is the one that absorbs slack.
        assert_eq!(widths.iter().map(|(_, w)| *w).collect::<Vec<_>>(), vec![400.0, 400.0]);
    }
}
