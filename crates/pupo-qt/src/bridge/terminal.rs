//! The terminal panel's bridge: the tabs, and the frame QML paints.
//!
//! Everything that *is* a terminal lives in `pupo_core::terminal` — the pty, the
//! VT engine, key encoding, selection and scrollback. This is only the seam: one
//! [`Terminal`] per tab, the engine's render state turned into lists QML can
//! bind to, and input handed back the other way.
//!
//! Tabs are kept per workspace rather than in one flat list. The panels are
//! built once and reused as the current workspace changes (see
//! `WorkspaceView.qml`), so a single list would show whichever project's shells
//! happened to be open when you last looked — flipping to another project and
//! back has to find the same prompts in the same directories.
//!
//! Nothing is spawned until the panel is first shown: a workspace whose terminal
//! you never open costs no shell, no reader thread and no scrollback buffer.

use std::collections::HashMap;

use pupo_core::terminal::keymap::KeyEvent;
use pupo_core::terminal::{CursorStyle, Grid, RenderState, SelectionMode, Terminal};
use pupo_core::theme::{terminal_theme, Rgb, DEFAULT_THEME};
use pupo_core::typography::terminal as sizes;
use pupo_core::{debug, remote};
use qmetaobject::*;

/// A wheel notch is 120 units of angle delta; three lines a notch is what every
/// other terminal on the desktop scrolls by, so 40 units to the line.
const WHEEL_UNITS_PER_LINE: i32 = 40;

/// Floors for the grid a shell is told about. A panel dragged down to nothing
/// still has to hand its child a size a curses program can lay out in.
const MIN_COLS: u16 = 4;
const MIN_ROWS: u16 = 2;

/// One tab: a shell, the label the strip shows, and whether it has rung.
struct Tab {
    id: u32,
    terminal: Terminal,
    label: String,
    /// Set by a BEL, cleared when the tab is looked at. A terminal that rang
    /// while you were reading another one is the whole point of the mark.
    bell: bool,
}

/// One workspace's tabs.
#[derive(Default)]
struct Pane {
    tabs: Vec<Tab>,
    current: u32,
    /// Numbers the labels: the first tab is "Terminal", the rest "Terminal#N".
    /// It counts opens rather than tabs, so closing #2 does not hand the next
    /// one its name, and resets only once the pane is empty.
    counter: u32,
}

impl Pane {
    fn index_of(&self, id: u32) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.id == id)
    }

    fn current_tab(&mut self) -> Option<&mut Tab> {
        let current = self.current;
        self.tabs.iter_mut().find(|tab| tab.id == current)
    }
}

#[derive(QObject, Default)]
pub struct TerminalBridge {
    base: qt_base_class!(trait QObject),

    /// The strip's tabs, left to right: `{ id, title, bell, closed }`. A plain
    /// list rebuilt whole rather than a model — a pane holds a handful of tabs,
    /// and a list that is rebuilt cannot disagree with itself.
    tabs: qt_property!(QVariantList; NOTIFY tabs_changed READ get_tabs),
    /// The current tab's id, or -1 when the pane has no tabs yet.
    current: qt_property!(i32; NOTIFY tabs_changed READ get_current),
    /// Whether this workspace's first shell has been spawned. The panel reads it
    /// to know whether it is still the lazy placeholder.
    started: qt_property!(bool; NOTIFY tabs_changed READ get_started),

    /// The visible grid, one entry per row: `{ runs, sel_start, sel_end }`, each
    /// run `{ text, col, cols, fg, bg, bold, italic, underline, strike }`. The engine
    /// has already collapsed the cells into runs and trimmed the blanks off the
    /// end, so a row is a handful of spans rather than eighty cells.
    rows: qt_property!(QVariantList; NOTIFY frame_changed READ get_rows),
    /// The cursor cell, and how to draw it: "block", "bar", "underline",
    /// "hollow" (the unfocused block), or "" when there is none to draw.
    cursor_col: qt_property!(i32; NOTIFY frame_changed READ get_cursor_col),
    cursor_row: qt_property!(i32; NOTIFY frame_changed READ get_cursor_row),
    cursor_style: qt_property!(QString; NOTIFY frame_changed READ get_cursor_style),
    /// The character the block cursor is covering. A filled block hides the
    /// glyph under it, so the panel redraws it in the background colour on top —
    /// which it can only do if it knows what it was.
    cursor_text: qt_property!(QString; NOTIFY frame_changed READ get_cursor_text),
    /// The terminal theme's own colours, which the panel's surface matches.
    background: qt_property!(QString; NOTIFY frame_changed READ get_background),
    foreground: qt_property!(QString; NOTIFY frame_changed READ get_foreground),
    /// How far back through the scrollback the viewport is, and how much there
    /// is of it. Both in lines; 0 offset is the live screen.
    scroll_offset: qt_property!(i32; NOTIFY frame_changed READ get_scroll_offset),
    scrollback: qt_property!(i32; NOTIFY frame_changed READ get_scrollback),

    tabs_changed: qt_signal!(),
    frame_changed: qt_signal!(),

    /// Which workspace's tabs to show. The panel binds it to `App.current`.
    set_workspace: qt_method!(fn(&mut self, key: QString)),
    /// Spawn this workspace's first shell if it has none. Called the first time
    /// the panel is actually shown, which is what makes the pane lazy.
    ensure_started: qt_method!(fn(&mut self)),

    add_tab: qt_method!(fn(&mut self)),
    close_tab: qt_method!(fn(&mut self, id: i32)),
    select_tab: qt_method!(fn(&mut self, id: i32)),
    move_tab: qt_method!(fn(&mut self, from: i32, to: i32)),

    /// Draw whatever the shells have written. Driven by a `Timer` in the panel:
    /// qmetaobject has no QTimer of its own, and a QML timer calling in is the
    /// idiomatic way round it.
    pump: qt_method!(fn(&mut self)),

    /// Qt's own key code, modifier flags and text, passed straight through —
    /// `pupo_core::terminal::keymap` speaks Qt's constants, so QML can hand over
    /// `event.key`, `event.modifiers` and `event.text` untranslated.
    key: qt_method!(fn(&mut self, key: i32, modifiers: i32, text: QString)),
    paste: qt_method!(fn(&mut self, text: QString)),
    /// The selected text, for the panel to put on the clipboard. Qt's clipboard
    /// has no binding here, so the copy itself happens in QML.
    copy_selection: qt_method!(fn(&self) -> QString),

    /// The grid, measured by the panel from the font it actually rendered. Only
    /// it knows what the glyphs came out as, and a shell told a size that does
    /// not match what is drawn gets every full-screen program wrong.
    resize: qt_method!(fn(&mut self, cols: i32, rows: i32, cell_w: i32, cell_h: i32)),
    /// A wheel event's `angleDelta.y`.
    wheel: qt_method!(fn(&mut self, delta: i32)),
    /// Scroll by whole lines, which is what a drag past the edge autoscrolls by.
    scroll_lines: qt_method!(fn(&mut self, lines: i32)),

    /// `mode` is "character", "word" or "line" — a single click, a double and a
    /// triple. Coordinates are viewport cells, not pixels.
    select_start: qt_method!(fn(&mut self, col: i32, row: i32, mode: QString)),
    select_extend: qt_method!(fn(&mut self, col: i32, row: i32)),
    select_clear: qt_method!(fn(&mut self)),

    /// Focus decides how the cursor is drawn and nothing else: two panes side by
    /// side must not both look active.
    set_focused: qt_method!(fn(&mut self, focused: bool)),

    set_theme: qt_method!(fn(&mut self, name: QString)),
    set_font_size: qt_method!(fn(&mut self, size: i32)),

    /// Stop every shell in every workspace. The window's teardown calls it, and
    /// it is idempotent.
    shutdown_all: qt_method!(fn(&mut self)),

    panes: HashMap<String, Pane>,
    workspace: String,
    /// Ids are unique across workspaces so a stale id from a strip that has not
    /// rebuilt yet can never name another workspace's tab.
    next_id: u32,
    /// The grid the panel measured, applied to a pane when it spawns and when it
    /// becomes the current one. Kept as its four numbers rather than a `Grid`
    /// so the whole object can still derive `Default`.
    cols: u16,
    rows_count: u16,
    cell_w: u16,
    cell_h: u16,
    theme_name: String,
    font_size: i32,
    focused: bool,
    /// The frame the getters answer from. `render()` needs the engine mutably
    /// and a property read only has `&self`, so the state is drawn during
    /// `pump` and kept here until the next one.
    frame: RenderState,
    cursor_char: String,
    scroll_offset_value: i32,
    scrollback_value: i32,
}

impl TerminalBridge {
    pub fn new() -> Self {
        Self {
            // Placeholders until the panel measures its font; a shell spawned
            // before that would print its first prompt at the wrong width.
            cols: 80,
            rows_count: 24,
            cell_w: 8,
            cell_h: 16,
            theme_name: DEFAULT_THEME.to_string(),
            font_size: sizes::DEFAULT_SIZE,
            next_id: 1,
            ..Default::default()
        }
    }

    // ---- panes ---------------------------------------------------------

    fn pane(&self) -> Option<&Pane> {
        self.panes.get(&self.workspace)
    }

    fn pane_mut(&mut self) -> Option<&mut Pane> {
        self.panes.get_mut(&self.workspace)
    }

    fn set_workspace(&mut self, key: QString) {
        let key = key.to_string();
        if key == self.workspace {
            return;
        }
        self.workspace = key;
        // The grid moved on while this pane was away — a panel resized under
        // another workspace resized nothing here, because a hidden pty must not
        // be told a size it cannot show.
        self.apply_grid();
        self.tabs_changed();
        self.refresh(true);
    }

    fn ensure_started(&mut self) {
        if self.workspace.is_empty() {
            return;
        }
        if self.pane().is_some_and(|pane| !pane.tabs.is_empty()) {
            return;
        }
        self.add_tab();
    }

    fn get_started(&self) -> bool {
        self.pane().is_some_and(|pane| !pane.tabs.is_empty())
    }

    // ---- tabs ----------------------------------------------------------

    fn get_tabs(&self) -> QVariantList {
        let mut list = QVariantList::default();
        let Some(pane) = self.pane() else {
            return list;
        };
        for tab in &pane.tabs {
            let mut map = QVariantMap::default();
            map.insert("id".into(), (tab.id as i32).into());
            map.insert("title".into(), QString::from(tab.label.as_str()).into());
            map.insert("bell".into(), tab.bell.into());
            map.insert("closed".into(), tab.terminal.is_closed().into());
            list.push(map.into());
        }
        list
    }

    fn get_current(&self) -> i32 {
        match self.pane() {
            Some(pane) if pane.index_of(pane.current).is_some() => pane.current as i32,
            _ => -1,
        }
    }

    fn add_tab(&mut self) {
        if self.workspace.is_empty() {
            return;
        }
        // A remote workspace's terminal is an ssh client to the far end, not a
        // shell in the anchor directory the workspace is keyed by.
        let target = remote::resolve(&self.workspace);
        let cwd = if target.is_some() {
            None
        } else {
            Some(self.workspace.clone())
        };
        let terminal = match Terminal::spawn(
            cwd.as_deref(),
            target.as_ref(),
            &self.theme_name,
            self.font_size,
            self.grid(),
        ) {
            Ok(terminal) => terminal,
            Err(error) => {
                debug::error("terminal.spawn-failed", &[("error", error.to_string())]);
                return;
            }
        };

        let id = self.next_id;
        self.next_id += 1;
        let focused = self.focused;
        let workspace = self.workspace.clone();
        let pane = self.panes.entry(workspace).or_default();
        pane.counter += 1;
        let label = if pane.counter == 1 {
            "Terminal".to_string()
        } else {
            format!("Terminal#{}", pane.counter)
        };
        let mut tab = Tab {
            id,
            terminal,
            label,
            bell: false,
        };
        tab.terminal.screen_mut().set_focused(focused);
        pane.tabs.push(tab);
        pane.current = id;
        debug::proc("terminal.tab-opened", &[("id", id.to_string())]);
        self.tabs_changed();
        self.refresh(true);
    }

    fn close_tab(&mut self, id: i32) {
        let Some(pane) = self.pane_mut() else { return };
        let Some(index) = pane.index_of(id as u32) else {
            return;
        };
        let mut tab = pane.tabs.remove(index);
        tab.terminal.shutdown();
        if pane.tabs.is_empty() {
            // Kraken opens a replacement rather than leaving an empty pane: a
            // terminal pane with no terminal in it is not a state the panel has
            // a face for. The browser's last ✕ closes the panel instead,
            // because a browser tab costs a renderer process and a shell does
            // not.
            pane.counter = 0;
            self.tabs_changed();
            self.add_tab();
            return;
        }
        if pane.current == id as u32 {
            let next = index.min(pane.tabs.len() - 1);
            pane.current = pane.tabs[next].id;
        }
        debug::proc("terminal.tab-closed", &[("id", id.to_string())]);
        self.tabs_changed();
        self.refresh(true);
    }

    fn select_tab(&mut self, id: i32) {
        let Some(pane) = self.pane_mut() else { return };
        let Some(index) = pane.index_of(id as u32) else {
            return;
        };
        pane.current = id as u32;
        pane.tabs[index].bell = false;
        self.tabs_changed();
        self.refresh(true);
    }

    fn move_tab(&mut self, from: i32, to: i32) {
        let Some(pane) = self.pane_mut() else { return };
        let count = pane.tabs.len();
        if from < 0 || to < 0 || from as usize >= count || to as usize >= count || from == to {
            return;
        }
        let tab = pane.tabs.remove(from as usize);
        pane.tabs.insert(to as usize, tab);
        self.tabs_changed();
    }

    // ---- the frame -----------------------------------------------------

    fn pump(&mut self) {
        let mut rang = false;
        let mut closed = false;
        // Every pane, not only the visible one: a background workspace's shell
        // keeps writing, and output nobody drains piles up in the channel it was
        // read into.
        for pane in self.panes.values_mut() {
            for tab in &mut pane.tabs {
                let was_closed = tab.terminal.is_closed();
                if tab.terminal.pump() {
                    if tab.terminal.take_bell() && tab.id != pane.current {
                        tab.bell = true;
                        rang = true;
                    }
                    closed |= tab.terminal.is_closed() != was_closed;
                }
            }
        }
        if rang || closed {
            self.tabs_changed();
        }
        self.refresh(false);
    }

    /// Rebuild the cached frame if the engine has drawn anything new. `force`
    /// covers the changes that are not the engine's — a new current tab, an
    /// emptied pane — where the state is the same but the frame QML is showing
    /// belongs to something else.
    fn refresh(&mut self, force: bool) {
        let snapshot = {
            let tab = self
                .panes
                .get_mut(&self.workspace)
                .and_then(Pane::current_tab);
            let Some(tab) = tab else {
                // No pane, or a pane whose tabs have all gone: show nothing
                // rather than the last frame of a terminal that is not there.
                //
                // Empty, but still in the theme's colours — a default
                // `RenderState` is black, which on a light theme paints the
                // panel a black rectangle for as long as it takes the shell to
                // start, and puts the "starting" notice on a ground it was
                // never coloured for.
                let theme = terminal_theme(&self.theme_name);
                self.frame = RenderState {
                    background: theme.background,
                    foreground: theme.foreground,
                    ..RenderState::default()
                };
                self.cursor_char.clear();
                self.scroll_offset_value = 0;
                self.scrollback_value = 0;
                self.frame_changed();
                return;
            };
            let offset = tab.terminal.screen().view_offset() as i32;
            let history = tab.terminal.screen().scrollback_len() as i32;
            // `render()` clears the dirty flag as it goes, so its own answer is
            // the only reliable one — asking the screen first and rendering
            // afterwards would drop a frame whenever the two disagreed.
            let state = tab.terminal.render();
            if !state.dirty && !force {
                return;
            }
            (state.clone(), offset, history)
        };
        let (state, offset, history) = snapshot;
        self.cursor_char = cursor_char(&state);
        self.frame = state;
        self.scroll_offset_value = offset;
        self.scrollback_value = history;
        self.frame_changed();
    }

    fn get_rows(&self) -> QVariantList {
        let mut list = QVariantList::default();
        for row in &self.frame.rows {
            let mut runs = QVariantList::default();
            let mut col = 0i32;
            for run in &row.runs {
                let mut map = QVariantMap::default();
                map.insert("text".into(), QString::from(run.text.as_str()).into());
                // The cell count, which is not the string's length once a run
                // carries anything outside the basic plane — and it is cells the
                // grid is laid out in.
                let cols = run.text.chars().count() as i32;
                // Where the run starts. Accumulated here rather than in the
                // panel, which would have to re-walk every row of every frame to
                // work out what the engine already knew.
                map.insert("col".into(), col.into());
                col += cols;
                map.insert("cols".into(), cols.into());
                map.insert("fg".into(), QString::from(hex(run.fg).as_str()).into());
                map.insert("bg".into(), QString::from(hex(run.bg).as_str()).into());
                map.insert("bold".into(), run.bold.into());
                map.insert("italic".into(), run.italic.into());
                map.insert("underline".into(), run.underline.into());
                map.insert("strike".into(), run.strike.into());
                runs.push(map.into());
            }
            let (start, end) = match row.selection {
                Some((start, end)) => (start as i32, end as i32),
                None => (-1, -1),
            };
            let mut map = QVariantMap::default();
            map.insert("runs".into(), runs.into());
            map.insert("sel_start".into(), start.into());
            map.insert("sel_end".into(), end.into());
            list.push(map.into());
        }
        list
    }

    fn get_cursor_col(&self) -> i32 {
        self.frame.cursor.map_or(-1, |cursor| cursor.col as i32)
    }

    fn get_cursor_row(&self) -> i32 {
        self.frame.cursor.map_or(-1, |cursor| cursor.row as i32)
    }

    fn get_cursor_style(&self) -> QString {
        let style = match self.frame.cursor {
            Some(cursor) => match cursor.style {
                CursorStyle::Bar => "bar",
                CursorStyle::Block => "block",
                CursorStyle::Underline => "underline",
                CursorStyle::HollowBlock => "hollow",
            },
            None => "",
        };
        style.into()
    }

    fn get_cursor_text(&self) -> QString {
        self.cursor_char.as_str().into()
    }

    fn get_background(&self) -> QString {
        hex(self.frame.background).as_str().into()
    }

    fn get_foreground(&self) -> QString {
        hex(self.frame.foreground).as_str().into()
    }

    fn get_scroll_offset(&self) -> i32 {
        self.scroll_offset_value
    }

    fn get_scrollback(&self) -> i32 {
        self.scrollback_value
    }

    // ---- input ---------------------------------------------------------

    fn with_current<R>(&mut self, action: impl FnOnce(&mut Terminal) -> R) -> Option<R> {
        let pane = self.panes.get_mut(&self.workspace)?;
        let tab = pane.current_tab()?;
        Some(action(&mut tab.terminal))
    }

    fn key(&mut self, key: i32, modifiers: i32, text: QString) {
        let text = text.to_string();
        let event = KeyEvent::new(key as u32, modifiers as u32, &text);
        // Only under `--debug-trace`: this is one record per keystroke, and a
        // log of everything typed is not something to write to disk unasked.
        // It is here rather than anywhere else because this is where key
        // encoding is got wrong — a chord that reaches the shell as the wrong
        // bytes looks identical from outside to one that never arrived.
        if debug::tracing_input() {
            debug::log(
                "terminal.key",
                &[
                    ("key", format!("0x{key:x}")),
                    ("mods", format!("0x{modifiers:x}")),
                    ("text", format!("{text:?}")),
                ],
            );
        }
        self.with_current(|terminal| terminal.key(&event));
        self.refresh(false);
    }

    fn paste(&mut self, text: QString) {
        let text = text.to_string();
        if text.is_empty() {
            return;
        }
        self.with_current(|terminal| terminal.paste(&text));
        self.refresh(false);
    }

    fn copy_selection(&self) -> QString {
        let Some(pane) = self.panes.get(&self.workspace) else {
            return QString::default();
        };
        let current = pane.current;
        match pane.tabs.iter().find(|tab| tab.id == current) {
            Some(tab) => tab.terminal.selection_text().as_str().into(),
            None => QString::default(),
        }
    }

    // ---- geometry ------------------------------------------------------

    fn resize(&mut self, cols: i32, rows: i32, cell_w: i32, cell_h: i32) {
        let clamp = |value: i32, min: u16| value.clamp(i32::from(min), i32::from(u16::MAX)) as u16;
        let (cols, rows) = (clamp(cols, MIN_COLS), clamp(rows, MIN_ROWS));
        let (cell_w, cell_h) = (clamp(cell_w, 1), clamp(cell_h, 1));
        if (cols, rows, cell_w, cell_h) == (self.cols, self.rows_count, self.cell_w, self.cell_h) {
            return;
        }
        self.cols = cols;
        self.rows_count = rows;
        self.cell_w = cell_w;
        self.cell_h = cell_h;
        self.apply_grid();
        self.refresh(true);
    }

    fn grid(&self) -> Grid {
        Grid::new(self.cols, self.rows_count, self.cell_w, self.cell_h)
    }

    /// Tell the current workspace's shells the grid. Only that one: resizing a
    /// pty for a pane nobody is looking at sends a SIGWINCH the program on the
    /// far end would redraw for at a size it is not being shown at.
    fn apply_grid(&mut self) {
        let grid = self.grid();
        let Some(pane) = self.panes.get_mut(&self.workspace) else {
            return;
        };
        for tab in &mut pane.tabs {
            tab.terminal
                .resize(grid.cols, grid.rows, grid.cell_w, grid.cell_h);
        }
    }

    fn wheel(&mut self, delta: i32) {
        self.scroll_lines(delta / WHEEL_UNITS_PER_LINE);
    }

    fn scroll_lines(&mut self, lines: i32) {
        if lines == 0 {
            return;
        }
        self.with_current(|terminal| terminal.screen_mut().scroll_viewport(-lines as isize));
        self.refresh(false);
    }

    // ---- selection -----------------------------------------------------

    fn select_start(&mut self, col: i32, row: i32, mode: QString) {
        let mode = match mode.to_string().as_str() {
            "word" => SelectionMode::Word,
            "line" => SelectionMode::Line,
            _ => SelectionMode::Character,
        };
        let (col, row) = (col.max(0) as usize, row.max(0) as usize);
        self.with_current(|terminal| terminal.screen_mut().selection_start(col, row, mode));
        self.refresh(false);
    }

    fn select_extend(&mut self, col: i32, row: i32) {
        let (col, row) = (col.max(0) as usize, row.max(0) as usize);
        self.with_current(|terminal| terminal.screen_mut().selection_extend(col, row));
        self.refresh(false);
    }

    fn select_clear(&mut self) {
        self.with_current(|terminal| terminal.screen_mut().selection_clear());
        self.refresh(false);
    }

    fn set_focused(&mut self, focused: bool) {
        if focused == self.focused {
            return;
        }
        self.focused = focused;
        // Every tab, not only the current one: a background tab brought forward
        // while the pane is unfocused has to come up outlined, not filled.
        for pane in self.panes.values_mut() {
            for tab in &mut pane.tabs {
                tab.terminal.screen_mut().set_focused(focused);
            }
        }
        self.refresh(false);
    }

    // ---- theme / lifecycle ---------------------------------------------

    fn set_theme(&mut self, name: QString) {
        let name = name.to_string();
        if name == self.theme_name {
            return;
        }
        self.theme_name = name;
        let theme = self.theme_name.clone();
        for pane in self.panes.values_mut() {
            for tab in &mut pane.tabs {
                tab.terminal.set_theme(&theme);
            }
        }
        self.refresh(true);
    }

    fn set_font_size(&mut self, size: i32) {
        let size = sizes::clamp(size);
        if size == self.font_size {
            return;
        }
        self.font_size = size;
        // The grid is not touched here: the panel re-measures the cell at the
        // new size and calls `resize` with what it found.
        for pane in self.panes.values_mut() {
            for tab in &mut pane.tabs {
                tab.terminal.set_font_size(size);
            }
        }
    }

    fn shutdown_all(&mut self) {
        for pane in self.panes.values_mut() {
            for tab in &mut pane.tabs {
                tab.terminal.shutdown();
            }
        }
    }
}

fn hex(color: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", color.0, color.1, color.2)
}

/// The character under the cursor, so a filled block can put it back in the
/// background colour. Runs are cells, not columns, so the row has to be walked.
fn cursor_char(state: &RenderState) -> String {
    let Some(cursor) = state.cursor else {
        return String::new();
    };
    let Some(row) = state.rows.get(cursor.row as usize) else {
        return String::new();
    };
    let mut col = 0usize;
    let target = cursor.col as usize;
    for run in &row.runs {
        let width = run.text.chars().count();
        if target < col + width {
            return run
                .text
                .chars()
                .nth(target - col)
                .filter(|ch| *ch != ' ')
                .map(String::from)
                .unwrap_or_default();
        }
        col += width;
    }
    String::new()
}
