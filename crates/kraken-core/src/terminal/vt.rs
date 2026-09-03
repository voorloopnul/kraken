//! The screen: a libghostty-vt terminal, and the render state read back out of it.
//!
//! Kraken linked libghostty-vt over ctypes for this. Kraken linked it again, this
//! time through the `libghostty-vt` crate, which builds Ghostty's own VT core
//! from source with Zig and links it statically — so there is one terminal
//! implementation behind Kraken and Ghostty rather than two that have to be kept
//! agreeing with each other.
//!
//! What that buys over a hand-written engine, beyond the escape sequences
//! Ghostty knows and this tree never implemented: scrollback that costs a
//! *memory budget* rather than a line count (see [`MAX_SCROLLBACK_BYTES`]),
//! history that reflows when the window is resized, and selection that
//! understands wrapped lines and shell prompts.
//!
//! The vocabulary the UI draws from is unchanged: [`RenderState`] is one entry
//! per visible row, each row's cells collapsed into runs of identical style
//! with colours already resolved against the active theme, plus the cursor and
//! a dirty flag so an unchanged frame costs nothing. Ghostty's grid is read
//! into that shape once per frame, and nothing above this module knows the
//! difference.

use std::cell::RefCell;
use std::rc::Rc;

use libghostty_vt::render::{
    CellIterator, CursorVisualStyle, RenderState as GRenderState, RowIterator,
};
use libghostty_vt::style::Underline;
use libghostty_vt::terminal::{
    Mode, Options as GOptions, Point as GPoint, PointCoordinate, ScrollViewport,
    Terminal as GTerminal,
};

use crate::theme::{Rgb, TerminalTheme};

/// Scrollback each terminal keeps, as a byte budget rather than a line count.
///
/// This is the one place the port changes a number's meaning, and it changes it
/// in the direction that was wanted: Ghostty prunes its page list to fit a size
/// in bytes, so what is configured here is the memory the history may cost
/// outright, instead of a line count whose cost then depends on how wide the
/// window happens to be.
///
/// 8 MiB holds roughly five thousand rows at 200 columns — the depth the
/// hand-written engine allocated 15 MB of fixed-width cells to guarantee.
pub const MAX_SCROLLBACK_BYTES: usize = 8 * 1024 * 1024;

/// libghostty's built-in palette (Tomorrow Night), used for the 16 named
/// colours whenever a theme does not override them — [`crate::theme::DARK`] is
/// exactly that case.
pub const DEFAULT_ANSI: [Rgb; 16] = [
    (0x1D, 0x1F, 0x21),
    (0xCC, 0x66, 0x66),
    (0xB5, 0xBD, 0x68),
    (0xF0, 0xC6, 0x74),
    (0x81, 0xA2, 0xBE),
    (0xB2, 0x94, 0xBB),
    (0x8A, 0xBE, 0xB7),
    (0xC5, 0xC8, 0xC6),
    (0x66, 0x66, 0x66),
    (0xD5, 0x4E, 0x53),
    (0xB9, 0xCA, 0x4A),
    (0xE7, 0xC5, 0x47),
    (0x7A, 0xA6, 0xDA),
    (0xC3, 0x97, 0xD8),
    (0x70, 0xC0, 0xB1),
    (0xEA, 0xEA, 0xEA),
];

/// How the cursor is drawn. The order is libghostty's, and the hollow block is
/// the unfocused variant of the block rather than something a program can ask
/// for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorStyle {
    Bar,
    #[default]
    Block,
    Underline,
    HollowBlock,
}

/// Mouse reporting, as the UI needs to know it: whether to report at all, and
/// in which encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MouseModes {
    /// 1000: report button press and release.
    pub buttons: bool,
    /// 1002: also report motion while a button is down.
    pub drag: bool,
    /// 1003: report every motion.
    pub any_motion: bool,
    /// 1006: SGR encoding, which is the only one that works past column 223.
    pub sgr: bool,
    /// 1004: report focus in/out.
    pub focus: bool,
}

impl MouseModes {
    /// Whether the program on the far end wants mouse events at all — when it
    /// does, the UI must stop treating a drag as a selection.
    pub fn reporting(&self) -> bool {
        self.buttons || self.drag || self.any_motion
    }
}

/// A run of cells sharing one style, with colours already resolved.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Run {
    pub text: String,
    pub fg: Rgb,
    pub bg: Rgb,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
}

/// One visible row, ready to paint.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Row {
    pub runs: Vec<Run>,
    /// Inclusive column range covered by the selection, if any. Selection is a
    /// highlight over the cells rather than part of their style, so it is kept
    /// beside the runs instead of splitting them.
    pub selection: Option<(u16, u16)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderCursor {
    pub col: u16,
    pub row: u16,
    pub style: CursorStyle,
}

/// Everything the UI needs for one frame.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RenderState {
    pub rows: Vec<Row>,
    /// `None` when the cursor is hidden, or when scrollback has scrolled it out
    /// of the viewport.
    pub cursor: Option<RenderCursor>,
    pub background: Rgb,
    pub foreground: Rgb,
    /// True when this frame differs from the one before it. A UI that sees
    /// `false` can skip the repaint entirely.
    pub dirty: bool,
}

/// How a drag grows a selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectionMode {
    #[default]
    Character,
    /// Double click: whole words at both ends.
    Word,
    /// Triple click: whole lines.
    Line,
}

/// A point in *screen* coordinates: row 0 is the oldest line still in the
/// scrollback, so a point keeps naming the same text as output scrolls past it.
///
/// This is Ghostty's `Point::Screen` space exactly, which is why selection
/// anchors survive pruning without any bookkeeping of our own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Point {
    pub row: usize,
    pub col: usize,
}

#[derive(Debug, Clone, Copy)]
struct Selection {
    anchor: Point,
    head: Point,
    mode: SelectionMode,
}

/// Characters a double-click keeps together. Terminals are used for paths,
/// flags and URLs, so a "word" is deliberately wider than a language's.
fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || "_-./~:@+".contains(ch)
}

/// What the VT stream asks of the embedder rather than of the grid: bytes to
/// write back, the bell, a new title.
///
/// Ghostty delivers these through callbacks installed on the terminal, which
/// outlive the call that registers them, so the state they touch is shared
/// rather than borrowed.
#[derive(Default)]
struct Effects {
    bell: bool,
    title: String,
    title_dirty: bool,
    responses: Vec<u8>,
}

/// The terminal screen and everything a VT sequence can do to it.
pub struct Screen {
    term: GTerminal<'static, 'static>,
    /// Ghostty's render state, kept across frames so it can report what
    /// actually changed instead of redrawing everything.
    render_state: GRenderState<'static>,
    effects: Rc<RefCell<Effects>>,

    cols: usize,
    rows: usize,

    focused: bool,
    selection: Option<Selection>,

    background: Rgb,
    foreground: Rgb,
    palette: Vec<Rgb>,

    dirty: bool,
    render: RenderState,
}

impl Screen {
    pub fn new(cols: usize, rows: usize, theme: &TerminalTheme) -> Self {
        let cols = cols.max(1);
        let rows = rows.max(1);

        let mut term = GTerminal::new(GOptions {
            cols: cols as u16,
            rows: rows as u16,
            max_scrollback: MAX_SCROLLBACK_BYTES,
        })
        .expect("libghostty terminal allocation");

        let effects = Rc::new(RefCell::new(Effects::default()));

        // Each handler owns a clone of the shared state, so the closures are
        // `'static` and the terminal can carry them for its whole life.
        let sink = Rc::clone(&effects);
        let registered = term
            .on_pty_write(move |_term, data| {
                sink.borrow_mut().responses.extend_from_slice(data);
            })
            .is_ok();
        debug_assert!(registered, "pty-write handler rejected by libghostty");

        let sink = Rc::clone(&effects);
        let registered = term
            .on_bell(move |_term| {
                sink.borrow_mut().bell = true;
            })
            .is_ok();
        debug_assert!(registered, "bell handler rejected by libghostty");

        let sink = Rc::clone(&effects);
        let registered = term
            .on_title_changed(move |term| {
                let title = term.title().unwrap_or_default().to_string();
                let mut effects = sink.borrow_mut();
                effects.title = title;
                effects.title_dirty = true;
            })
            .is_ok();
        debug_assert!(registered, "title handler rejected by libghostty");

        let mut screen = Self {
            term,
            render_state: GRenderState::new().expect("libghostty render state allocation"),
            effects,
            cols,
            rows,
            focused: true,
            selection: None,
            background: theme.background,
            foreground: theme.foreground,
            palette: palette_for(theme),
            dirty: true,
            render: RenderState::default(),
        };
        screen.push_theme();
        screen
    }

    /// Hand the theme to Ghostty, which is what resolves a cell's colour.
    ///
    /// The palette has to go across too, not just the two defaults: cells hold
    /// a palette *index* until they are rendered, and Ghostty resolves that
    /// against the palette it was given. Pushing only the foreground and
    /// background would leave every `SGR 31` cell painted in Ghostty's built-in
    /// red no matter which theme Kraken is on.
    fn push_theme(&mut self) {
        let fg = self.foreground;
        let bg = self.background;
        let _ = self.term.set_default_fg_color(Some(rgb_to_ghostty(fg)));
        let _ = self.term.set_default_bg_color(Some(rgb_to_ghostty(bg)));

        if let Ok(mut palette) = self.term.default_color_palette() {
            for (index, colour) in self.palette.iter().take(256).enumerate() {
                palette.set(
                    libghostty_vt::style::PaletteIndex(index as u8),
                    rgb_to_ghostty(*colour),
                );
            }
            let _ = self.term.set_default_color_palette(Some(palette));
        }
    }

    // ---- geometry ------------------------------------------------------

    pub fn cols(&self) -> usize {
        self.cols
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    /// Resize the grid. Ghostty reflows the history to the new width, so a
    /// window that grows back reads the way it did before it shrank — which the
    /// hand-written engine could not do.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let cols = cols.max(1);
        let rows = rows.max(1);
        if cols == self.cols && rows == self.rows {
            return;
        }
        // Cell pixel metrics only matter to features Kraken does not use (kitty
        // graphics placement); the grid itself is sized in cells.
        if self.term.resize(cols as u16, rows as u16, 1, 1).is_err() {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        // Reflow moves text under any anchor we were holding.
        self.selection = None;
        self.dirty = true;
    }

    // ---- theme ---------------------------------------------------------

    /// Repoint the engine at another theme. Cell colours are symbolic until
    /// render time, so this only has to invalidate the frame.
    pub fn set_theme(&mut self, theme: &TerminalTheme) {
        self.background = theme.background;
        self.foreground = theme.foreground;
        self.palette = palette_for(theme);
        self.push_theme();
        self.dirty = true;
    }

    // ---- modes the UI cares about --------------------------------------

    fn mode(&self, mode: Mode) -> bool {
        self.term.mode(mode).unwrap_or(false)
    }

    pub fn bracketed_paste(&self) -> bool {
        self.mode(Mode::BRACKETED_PASTE)
    }

    pub fn app_cursor(&self) -> bool {
        self.mode(Mode::DECCKM)
    }

    pub fn app_keypad(&self) -> bool {
        self.mode(Mode::KEYPAD_KEYS)
    }

    pub fn mouse_modes(&self) -> MouseModes {
        MouseModes {
            buttons: self.mode(Mode::NORMAL_MOUSE),
            drag: self.mode(Mode::BUTTON_MOUSE),
            any_motion: self.mode(Mode::ANY_MOUSE),
            sgr: self.mode(Mode::SGR_MOUSE),
            focus: self.mode(Mode::FOCUS_EVENT),
        }
    }

    pub fn autowrap(&self) -> bool {
        self.mode(Mode::WRAPAROUND)
    }

    pub fn on_alt_screen(&self) -> bool {
        self.mode(Mode::ALT_SCREEN_SAVE)
            || self.mode(Mode::ALT_SCREEN)
            || self.mode(Mode::ALT_SCREEN_LEGACY)
    }

    // ---- what the far end asked for ------------------------------------

    pub fn title(&self) -> String {
        self.effects.borrow().title.clone()
    }

    /// The title, if it changed since the last call. The UI renames a tab from
    /// this, so it must not be told about a title it already has.
    pub fn take_title(&mut self) -> Option<String> {
        let mut effects = self.effects.borrow_mut();
        if !effects.title_dirty {
            return None;
        }
        effects.title_dirty = false;
        Some(effects.title.clone())
    }

    pub fn take_bell(&mut self) -> bool {
        std::mem::take(&mut self.effects.borrow_mut().bell)
    }

    pub fn take_responses(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.effects.borrow_mut().responses)
    }

    /// Focus drives the cursor's appearance, and — when the far end asked for
    /// focus reporting — a report back to it.
    pub fn set_focused(&mut self, focused: bool) {
        if self.focused == focused {
            return;
        }
        self.focused = focused;
        if self.mouse_modes().focus {
            let report: &[u8] = if focused { b"\x1b[I" } else { b"\x1b[O" };
            self.effects
                .borrow_mut()
                .responses
                .extend_from_slice(report);
        }
        self.dirty = true;
    }

    // ---- cursor --------------------------------------------------------

    pub fn cursor(&self) -> (usize, usize) {
        (
            self.term.cursor_x().unwrap_or(0) as usize,
            self.term.cursor_y().unwrap_or(0) as usize,
        )
    }

    pub fn cursor_visible(&self) -> bool {
        self.term.is_cursor_visible().unwrap_or(true)
    }

    /// The cursor's shape.
    ///
    /// Ghostty resolves the shape while building a render state — `DECSCUSR`
    /// and the unfocused hollow block are both decided there — so this has to
    /// bring the frame up to date before reading it, and takes `&mut self` for
    /// that. (The terminal's own `cursor_style` is the SGR pen, and a different
    /// thing entirely.)
    pub fn cursor_style(&mut self) -> CursorStyle {
        self.render();
        self.render
            .cursor
            .map_or(CursorStyle::Block, |cursor| cursor.style)
    }

    // ---- scrollback ----------------------------------------------------

    pub fn scrollback_len(&self) -> usize {
        self.term.scrollback_rows().unwrap_or(0)
    }

    /// Lines scrolled back from the bottom of the history.
    pub fn view_offset(&self) -> usize {
        // Ghostty tracks the viewport itself and reports it the way a scrollbar
        // wants it: how far down the whole scrollable area the viewport sits.
        // What the UI asks for here is the distance from the *bottom*.
        let bar = self.scrollbar();
        let bottom = (bar.total as usize).saturating_sub(bar.len as usize);
        bottom.saturating_sub(bar.offset as usize)
    }

    /// Screen row of the first visible line.
    fn viewport_top(&self) -> usize {
        self.scrollbar().offset as usize
    }

    fn scrollbar(&self) -> libghostty_vt::terminal::Scrollbar {
        self.term.scrollbar().unwrap_or_default()
    }

    pub fn scroll_viewport(&mut self, delta: isize) {
        if delta == 0 {
            return;
        }
        self.term.scroll_viewport(ScrollViewport::Delta(delta));
        self.dirty = true;
    }

    pub fn scroll_to_bottom(&mut self) {
        self.term.scroll_viewport(ScrollViewport::Bottom);
        self.dirty = true;
    }

    pub fn scroll_to_top(&mut self) {
        self.term.scroll_viewport(ScrollViewport::Top);
        self.dirty = true;
    }

    // ---- text ----------------------------------------------------------

    /// Visible row `y` as text, trailing blanks trimmed.
    ///
    /// Read straight off the grid rather than out of the last rendered frame,
    /// so it answers for the terminal's state now and not for whatever was
    /// painted last.
    pub fn row_text(&self, y: usize) -> String {
        let mut text = String::new();
        for x in 0..self.cols {
            let cell = self
                .term
                .grid_ref(GPoint::Viewport(PointCoordinate {
                    x: x as u16,
                    y: y as u32,
                }))
                .ok()
                .and_then(|grid_ref| grid_ref.cell().ok());
            let ch = cell
                .and_then(|cell| cell.codepoint().ok())
                .and_then(char::from_u32)
                .unwrap_or(' ');
            text.push(if ch == '\0' { ' ' } else { ch });
        }
        while text.ends_with(' ') {
            text.pop();
        }
        text
    }

    /// The whole viewport as text, one row per line, trailing blank rows kept
    /// (the grid has a fixed height and a test that asserts on row 3 should not
    /// have to care whether rows 4-24 exist).
    pub fn viewport_text(&self) -> String {
        (0..self.rows)
            .map(|y| self.row_text(y))
            .collect::<Vec<_>>()
            .join("\n")
    }

    // ---- selection -----------------------------------------------------

    /// Screen coordinates for a viewport cell, which is what the UI has after
    /// dividing a mouse position by the cell size.
    pub fn viewport_to_screen(&self, col: usize, row: usize) -> Point {
        Point {
            row: self.viewport_top() + row.min(self.rows.saturating_sub(1)),
            col: col.min(self.cols.saturating_sub(1)),
        }
    }

    pub fn selection_start(&mut self, col: usize, row: usize, mode: SelectionMode) {
        let point = self.viewport_to_screen(col, row);
        self.selection = Some(Selection {
            anchor: point,
            head: point,
            mode,
        });
        self.dirty = true;
    }

    pub fn selection_extend(&mut self, col: usize, row: usize) {
        let point = self.viewport_to_screen(col, row);
        if let Some(selection) = self.selection.as_mut() {
            if selection.head != point {
                selection.head = point;
                self.dirty = true;
            }
        }
    }

    pub fn selection_clear(&mut self) {
        if self.selection.take().is_some() {
            self.dirty = true;
        }
    }

    pub fn has_selection(&self) -> bool {
        self.selection.is_some()
    }

    /// Text of one screen row, trailing blanks trimmed.
    fn screen_row_text(&self, row: usize) -> String {
        let mut text = String::new();
        for x in 0..self.cols {
            let ch = self
                .term
                .grid_ref(GPoint::Screen(PointCoordinate {
                    x: x as u16,
                    y: row as u32,
                }))
                .ok()
                .and_then(|grid_ref| grid_ref.cell().ok())
                .and_then(|cell| cell.codepoint().ok())
                .and_then(char::from_u32)
                .unwrap_or(' ');
            text.push(if ch == '\0' { ' ' } else { ch });
        }
        text
    }

    /// The selection's endpoints in reading order, with word and line modes
    /// already grown out to their boundaries.
    fn selection_range(&self) -> Option<(Point, Point)> {
        let selection = self.selection?;
        let (mut start, mut end) = if selection.anchor <= selection.head {
            (selection.anchor, selection.head)
        } else {
            (selection.head, selection.anchor)
        };
        match selection.mode {
            SelectionMode::Character => {}
            SelectionMode::Word => {
                let first = self.screen_row_text(start.row);
                let last = self.screen_row_text(end.row);
                let chars: Vec<char> = first.chars().collect();
                if chars.get(start.col).copied().is_some_and(is_word_char) {
                    while start.col > 0
                        && chars.get(start.col - 1).copied().is_some_and(is_word_char)
                    {
                        start.col -= 1;
                    }
                }
                let chars: Vec<char> = last.chars().collect();
                if chars.get(end.col).copied().is_some_and(is_word_char) {
                    while end.col + 1 < self.cols
                        && chars.get(end.col + 1).copied().is_some_and(is_word_char)
                    {
                        end.col += 1;
                    }
                }
            }
            SelectionMode::Line => {
                start.col = 0;
                end.col = self.cols.saturating_sub(1);
            }
        }
        Some((start, end))
    }

    pub fn selection_text(&self) -> String {
        let Some((start, end)) = self.selection_range() else {
            return String::new();
        };
        let mut text = String::new();
        for row in start.row..=end.row {
            let source = self.screen_row_text(row);
            let chars: Vec<char> = source.chars().collect();
            let from = if row == start.row { start.col } else { 0 };
            let to = if row == end.row {
                end.col
            } else {
                self.cols.saturating_sub(1)
            };
            if from > to {
                continue;
            }
            let mut line: String = chars
                .get(from..=to.min(chars.len().saturating_sub(1)))
                .map(|slice| slice.iter().collect())
                .unwrap_or_default();
            while line.ends_with(' ') {
                line.pop();
            }
            text.push_str(&line);
            // A row the terminal wrapped is one line of the user's, so it is
            // joined to the next rather than broken by a newline they never
            // typed. The last row of the selection ends the text either way.
            if row < end.row && !self.row_is_wrapped(row) {
                text.push('\n');
            }
        }
        text
    }

    /// Whether a screen row ran out of columns and continues on the next one.
    fn row_is_wrapped(&self, row: usize) -> bool {
        self.term
            .grid_ref(GPoint::Screen(PointCoordinate {
                x: 0,
                y: row as u32,
            }))
            .ok()
            .and_then(|grid_ref| grid_ref.row().ok())
            .and_then(|row| row.is_wrapped().ok())
            .unwrap_or(false)
    }

    // ---- rendering -----------------------------------------------------

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// The frame to paint. Rebuilt only when something changed; the returned
    /// state's `dirty` says whether it did.
    pub fn render(&mut self) -> &RenderState {
        if !self.dirty {
            self.render.dirty = false;
            return &self.render;
        }

        let range = self.selection_range();
        let viewport_top = self.viewport_top();
        let cols = self.cols;
        let default_fg = self.foreground;
        let default_bg = self.background;

        let mut rows: Vec<Row> = Vec::with_capacity(self.rows);
        let cursor;

        // Scoped so the snapshot's borrow of the render state ends before the
        // frame is stored back on `self`.
        {
            let snapshot = match self.render_state.update(&self.term) {
                Ok(snapshot) => snapshot,
                Err(_) => return &self.render,
            };

            cursor = snapshot
                .cursor_visible()
                .unwrap_or(false)
                .then(|| snapshot.cursor_viewport().ok().flatten())
                .flatten()
                .map(|position| RenderCursor {
                    col: position.x,
                    row: position.y,
                    style: if self.focused {
                        match snapshot.cursor_visual_style() {
                            Ok(CursorVisualStyle::Bar) => CursorStyle::Bar,
                            Ok(CursorVisualStyle::Underline) => CursorStyle::Underline,
                            Ok(CursorVisualStyle::BlockHollow) => CursorStyle::HollowBlock,
                            _ => CursorStyle::Block,
                        }
                    } else {
                        // An unfocused terminal outlines the cell instead of
                        // filling it, so two panes side by side cannot both
                        // look active.
                        CursorStyle::HollowBlock
                    },
                });

            let mut row_iter = match RowIterator::new() {
                Ok(iter) => iter,
                Err(_) => return &self.render,
            };
            let mut cell_iter = match CellIterator::new() {
                Ok(iter) => iter,
                Err(_) => return &self.render,
            };

            let mut iteration = match row_iter.update(&snapshot) {
                Ok(iteration) => iteration,
                Err(_) => return &self.render,
            };

            let mut y = 0usize;
            while let Some(row_ref) = iteration.next() {
                let selection = range
                    .and_then(|(from, to)| row_selection(viewport_top + y, cols, from, to));
                let mut row = Row {
                    runs: Vec::new(),
                    selection,
                };
                if let Ok(mut cells) = cell_iter.update(row_ref) {
                    while let Some(cell) = cells.next() {
                        let style = cell.style().unwrap_or_default();
                        let mut text = String::new();
                        if cell.graphemes_utf8(&mut text).is_err() || text.is_empty() {
                            text.push(' ');
                        }
                        let mut fg = cell
                            .fg_color()
                            .ok()
                            .flatten()
                            .map_or(default_fg, ghostty_to_rgb);
                        let mut bg = cell
                            .bg_color()
                            .ok()
                            .flatten()
                            .map_or(default_bg, ghostty_to_rgb);
                        // Ghostty reports the cell's own colours and leaves
                        // SGR 7 as a flag, because what inverse means depends
                        // on the defaults the embedder resolved them against.
                        if style.inverse {
                            std::mem::swap(&mut fg, &mut bg);
                        }
                        let candidate = Run {
                            text,
                            fg,
                            bg,
                            bold: style.bold,
                            italic: style.italic,
                            underline: style.underline != Underline::None,
                            strike: style.strikethrough,
                        };
                        match row.runs.last_mut() {
                            Some(previous) if previous.matches(&candidate) => {
                                previous.text.push_str(&candidate.text);
                            }
                            _ => row.runs.push(candidate),
                        }
                    }
                }
                trim_blank_tail(&mut row.runs, default_bg);
                rows.push(row);
                y += 1;
            }
        }

        self.render = RenderState {
            rows,
            cursor,
            background: default_bg,
            foreground: default_fg,
            dirty: true,
        };
        self.dirty = false;
        &self.render
    }

    pub fn palette(&self) -> &[Rgb] {
        &self.palette
    }
}

impl Run {
    /// Whether another cell's style is the same, and so belongs in this run.
    fn matches(&self, other: &Run) -> bool {
        self.fg == other.fg
            && self.bg == other.bg
            && self.bold == other.bold
            && self.italic == other.italic
            && self.underline == other.underline
            && self.strike == other.strike
    }
}

/// Drop the run of blank cells a row usually ends in.
///
/// A row is 80-odd cells wide and most of them are usually nothing at all.
/// Trailing blanks that would paint the background onto the background save the
/// UI a text item — sometimes a whole run — per row. Anything with a colour or a
/// rule on it stays, because that *is* visible: `\x1b[41m\x1b[K` paints a red
/// bar to the end of the line and the cells carrying it are all spaces.
fn trim_blank_tail(runs: &mut Vec<Run>, background: Rgb) {
    if let Some(run) = runs.last_mut() {
        if run.bg == background && !run.underline && !run.strike {
            while run.text.ends_with(' ') {
                run.text.pop();
            }
            if run.text.is_empty() {
                runs.pop();
            }
        }
    }
}

/// The columns of one screen row covered by a selection, if any.
fn row_selection(row: usize, cols: usize, start: Point, end: Point) -> Option<(u16, u16)> {
    if row < start.row || row > end.row {
        return None;
    }
    let from = if row == start.row { start.col } else { 0 };
    let to = if row == end.row {
        end.col
    } else {
        cols.saturating_sub(1)
    };
    if from > to {
        return None;
    }
    Some((from as u16, to.min(cols.saturating_sub(1)) as u16))
}

fn rgb_to_ghostty(color: Rgb) -> libghostty_vt::style::RgbColor {
    libghostty_vt::style::RgbColor {
        r: color.0,
        g: color.1,
        b: color.2,
    }
}

fn ghostty_to_rgb(color: libghostty_vt::style::RgbColor) -> Rgb {
    (color.r, color.g, color.b)
}

fn palette_for(theme: &TerminalTheme) -> Vec<Rgb> {
    theme.palette256().unwrap_or_else(|| {
        // Built from the same cube and ramp formulas as any other palette, so
        // only the 16 named colours differ.
        let fallback = TerminalTheme {
            name: theme.name,
            background: theme.background,
            foreground: theme.foreground,
            ansi: Some(DEFAULT_ANSI),
        };
        fallback
            .palette256()
            .unwrap_or_else(|| vec![theme.foreground; 256])
    })
}

/// The terminal and the bytes driven into it.
///
/// Kept as a type of its own because the hand-written engine had a parser to
/// hold beside the screen. Ghostty parses inside its own terminal, so this is
/// now a thin pass-through — but it stays, because every caller and every test
/// in the tree is written against it.
pub struct Vt {
    screen: Screen,
}

impl Vt {
    pub fn new(cols: usize, rows: usize, theme: &TerminalTheme) -> Self {
        Self {
            screen: Screen::new(cols, rows, theme),
        }
    }

    /// Drive the terminal with bytes from the pty — or, in a test, with a byte
    /// string written by hand.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.screen.term.vt_write(bytes);
        self.screen.dirty = true;
    }

    pub fn screen(&self) -> &Screen {
        &self.screen
    }

    pub fn screen_mut(&mut self) -> &mut Screen {
        &mut self.screen
    }
}

impl std::ops::Deref for Vt {
    type Target = Screen;

    fn deref(&self) -> &Screen {
        &self.screen
    }
}

impl std::ops::DerefMut for Vt {
    fn deref_mut(&mut self) -> &mut Screen {
        &mut self.screen
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{DARK, LIGHT};

    fn vt(cols: usize, rows: usize) -> Vt {
        Vt::new(cols, rows, &DARK)
    }

    fn row_texts(vt: &Vt, count: usize) -> Vec<String> {
        (0..count).map(|y| vt.row_text(y)).collect()
    }

    #[test]
    fn plain_text_lands_on_the_first_row() {
        let mut vt = vt(20, 5);
        vt.feed(b"hello");
        assert_eq!(vt.row_text(0), "hello");
        assert_eq!(vt.cursor(), (5, 0));
    }

    #[test]
    fn text_wraps_at_the_last_column_and_marks_the_line() {
        let mut vt = vt(5, 3);
        vt.feed(b"abcdefgh");
        assert_eq!(row_texts(&vt, 2), vec!["abcde", "fgh"]);
        // The last column can be written without scrolling: the wrap only
        // happens once there is another character to place.
        let mut edge = super::tests::vt(5, 3);
        edge.feed(b"abcde");
        assert_eq!(edge.cursor(), (4, 0));
    }

    #[test]
    fn autowrap_off_overwrites_the_last_column() {
        let mut vt = vt(5, 3);
        vt.feed(b"\x1b[?7l");
        vt.feed(b"abcdefgh");
        assert_eq!(vt.row_text(0), "abcdh");
        assert_eq!(vt.row_text(1), "");
    }

    #[test]
    fn carriage_return_linefeed_backspace_and_tab_move_the_cursor() {
        let mut vt = vt(20, 4);
        vt.feed(b"one\r\ntwo");
        assert_eq!(row_texts(&vt, 2), vec!["one", "two"]);

        vt.feed(b"\x08X");
        assert_eq!(vt.row_text(1), "twX");

        vt.feed(b"\r\n\ta");
        assert_eq!(vt.row_text(2), "        a");
    }

    #[test]
    fn sgr_sets_and_clears_attributes() {
        let mut vt = vt(20, 2);
        vt.feed(b"\x1b[1;3;4;9mstyled\x1b[0m plain");
        let state = vt.render().clone();
        let runs = &state.rows[0].runs;
        assert_eq!(runs[0].text, "styled");
        assert!(runs[0].bold && runs[0].italic && runs[0].underline && runs[0].strike);
        assert_eq!(runs[1].text, " plain");
        assert!(!runs[1].bold && !runs[1].underline);
    }

    #[test]
    fn the_three_colour_depths_all_resolve_against_the_theme() {
        let mut vt = Vt::new(30, 2, &LIGHT);
        vt.feed(b"\x1b[31mA\x1b[38;5;196mB\x1b[38;2;10;20;30mC");
        let state = vt.render().clone();
        let runs = &state.rows[0].runs;
        // Named colours come from the theme's own ANSI table.
        assert_eq!(runs[0].fg, LIGHT.ansi.expect("light overrides ANSI")[1]);
        // The 256-colour cube is shared by every theme.
        assert_eq!(runs[1].fg, LIGHT.palette256().unwrap()[196]);
        assert_eq!(runs[2].fg, (10, 20, 30));
    }

    #[test]
    fn a_theme_without_an_ansi_table_falls_back_to_the_builtin_palette() {
        let mut vt = vt(10, 2);
        vt.feed(b"\x1b[32mgreen");
        assert_eq!(vt.render().rows[0].runs[0].fg, DEFAULT_ANSI[2]);
    }

    #[test]
    fn the_colon_form_of_truecolor_is_understood_too() {
        let mut vt = vt(20, 2);
        vt.feed(b"\x1b[38:2::1:2:3mx");
        assert_eq!(vt.render().rows[0].runs[0].fg, (1, 2, 3));
    }

    #[test]
    fn inverse_swaps_the_resolved_colours() {
        let mut vt = Vt::new(10, 2, &LIGHT);
        vt.feed(b"\x1b[7mx");
        let run = vt.render().rows[0].runs[0].clone();
        assert_eq!(run.fg, LIGHT.background);
        assert_eq!(run.bg, LIGHT.foreground);
    }

    #[test]
    fn cursor_addressing_puts_text_where_it_was_asked_to() {
        let mut vt = vt(20, 5);
        vt.feed(b"\x1b[3;5Hhere");
        assert_eq!(vt.row_text(2), "    here");
        vt.feed(b"\x1b[HA");
        assert_eq!(vt.row_text(0), "A");
        // Relative moves, and a column-absolute one.
        vt.feed(b"\x1b[2B\x1b[10GB");
        assert_eq!(vt.row_text(2), "    here B");
    }

    #[test]
    fn erase_in_line_clears_the_three_halves() {
        let mut vt = vt(10, 3);
        vt.feed(b"abcdefghij\x1b[1;5H\x1b[K");
        assert_eq!(vt.row_text(0), "abcd");

        vt.feed(b"\x1b[1;1Habcdefghij\x1b[1;5H\x1b[1K");
        assert_eq!(vt.row_text(0), "     fghij");

        vt.feed(b"\x1b[2K");
        assert_eq!(vt.row_text(0), "");
    }

    #[test]
    fn erase_in_display_clears_below_above_and_everything() {
        let mut vt = vt(10, 4);
        vt.feed(b"one\r\ntwo\r\nthree\r\nfour");
        vt.feed(b"\x1b[2;2H\x1b[J");
        assert_eq!(row_texts(&vt, 4), vec!["one", "t", "", ""]);

        vt.feed(b"\x1b[2J");
        assert_eq!(vt.viewport_text().trim(), "");
    }

    #[test]
    fn a_scroll_region_confines_scrolling_to_its_own_rows() {
        let mut vt = vt(10, 5);
        vt.feed(b"a\r\nb\r\nc\r\nd\r\ne");
        // Rows 2..4 scroll; the first and last stay put.
        vt.feed(b"\x1b[2;4r\x1b[4;1H\n");
        assert_eq!(row_texts(&vt, 5), vec!["a", "c", "d", "", "e"]);
        // Nothing left the screen, so nothing entered the scrollback.
        assert_eq!(vt.scrollback_len(), 0);
    }

    #[test]
    fn lines_that_scroll_off_the_top_land_in_the_scrollback() {
        let mut vt = vt(10, 3);
        for i in 0..6 {
            vt.feed(format!("line{i}\r\n").as_bytes());
        }
        assert_eq!(vt.scrollback_len(), 4);
        assert_eq!(row_texts(&vt, 3), vec!["line4", "line5", ""]);

        vt.screen_mut().scroll_viewport(-2);
        assert_eq!(vt.view_offset(), 2);
        assert_eq!(row_texts(&vt, 3), vec!["line2", "line3", "line4"]);

        vt.screen_mut().scroll_to_bottom();
        assert_eq!(row_texts(&vt, 3), vec!["line4", "line5", ""]);
    }

    /// The bound is a memory budget now, not a line count, so what this can
    /// assert is that history stops growing and that the *newest* lines are the
    /// ones kept — which is the property the old line-count bound was really
    /// there to protect.
    #[test]
    fn the_scrollback_stops_at_its_budget_and_keeps_the_newest_lines() {
        let mut vt = vt(10, 2);
        let total = 200_000;
        // Fed in one buffer rather than a call per line: the budget needs a lot
        // of lines to fill, and this is a test of pruning, not of call overhead.
        let mut bulk = String::new();
        for i in 0..total {
            bulk.push_str(&format!("{i}\r\n"));
        }
        vt.feed(bulk.as_bytes());
        let held = vt.scrollback_len();
        assert!(
            held > 0 && held < total,
            "history should be pruned to a budget, not unbounded and not empty: {held}"
        );

        // Feeding more must not grow it further.
        let mut bulk = String::new();
        for i in total..(total + 10_000) {
            bulk.push_str(&format!("{i}\r\n"));
        }
        vt.feed(bulk.as_bytes());
        let after = vt.scrollback_len();
        assert!(
            after.abs_diff(held) * 10 < held,
            "history kept growing past its budget: {held} then {after}"
        );

        // The last line written is still on screen; the first is long gone.
        vt.screen_mut().scroll_to_bottom();
        assert_eq!(vt.screen().row_text(0), format!("{}", total + 10_000 - 1));
        vt.screen_mut().scroll_to_top();
        assert_ne!(vt.screen().row_text(0), "0");
    }

    #[test]
    fn reading_history_holds_still_while_output_arrives() {
        let mut vt = vt(10, 3);
        for i in 0..10 {
            vt.feed(format!("line{i}\r\n").as_bytes());
        }
        vt.screen_mut().scroll_viewport(-3);
        let seen = row_texts(&vt, 3);
        vt.feed(b"more\r\n");
        assert_eq!(row_texts(&vt, 3), seen);
    }

    #[test]
    fn the_alternate_screen_is_a_scratch_surface_the_primary_survives() {
        let mut vt = vt(10, 3);
        vt.feed(b"shell\r\n");
        vt.feed(b"\x1b[?1049h");
        assert!(vt.on_alt_screen());
        vt.feed(b"fullscreen");
        // DECSET 1049 saves the cursor and clears the alternate screen, but it
        // does not *home* the cursor — so the text lands on the row the shell
        // had left it on. (The hand-written engine homed it, and this assertion
        // used to read row 0.)
        assert_eq!(vt.row_text(1), "fullscreen");
        // Nothing a full-screen program scrolls off is history.
        vt.feed(b"\r\n\r\n\r\n\r\n");
        assert_eq!(vt.scrollback_len(), 0);

        vt.feed(b"\x1b[?1049l");
        assert!(!vt.on_alt_screen());
        assert_eq!(vt.row_text(0), "shell");
        assert_eq!(vt.cursor(), (0, 1));
    }

    #[test]
    fn bracketed_paste_and_the_application_modes_are_reported_to_the_ui() {
        let mut vt = vt(10, 3);
        assert!(!vt.bracketed_paste() && !vt.app_cursor());
        vt.feed(b"\x1b[?2004h\x1b[?1h\x1b[?1000h\x1b[?1006h");
        assert!(vt.bracketed_paste());
        assert!(vt.app_cursor());
        assert!(vt.mouse_modes().reporting() && vt.mouse_modes().sgr);
        vt.feed(b"\x1b[?2004l\x1b[?1l");
        assert!(!vt.bracketed_paste() && !vt.app_cursor());
    }

    #[test]
    fn the_window_title_comes_out_of_an_osc() {
        let mut vt = vt(10, 3);
        vt.feed(b"\x1b]0;kraken \xe2\x80\x94 build\x07");
        assert_eq!(vt.title(), "kraken — build");
        assert_eq!(vt.screen_mut().take_title().as_deref(), Some("kraken — build"));
        // Only the edge is reported; an unchanged title is nothing to do.
        assert_eq!(vt.screen_mut().take_title(), None);
    }

    #[test]
    fn the_bell_is_a_signal_the_ui_can_take_once() {
        let mut vt = vt(10, 3);
        vt.feed(b"ding\x07");
        assert!(vt.screen_mut().take_bell());
        assert!(!vt.screen_mut().take_bell());
    }

    #[test]
    fn a_cursor_position_report_goes_back_to_the_program() {
        let mut vt = vt(20, 5);
        vt.feed(b"\x1b[3;7H\x1b[6n");
        assert_eq!(vt.screen_mut().take_responses(), b"\x1b[3;7R".to_vec());
        assert!(vt.screen_mut().take_responses().is_empty());
    }

    #[test]
    fn the_cursor_hides_shows_and_changes_shape() {
        let mut vt = vt(10, 3);
        assert_eq!(vt.cursor_style(), CursorStyle::Block);
        vt.feed(b"\x1b[5 q");
        assert_eq!(vt.cursor_style(), CursorStyle::Bar);
        vt.feed(b"\x1b[3 q");
        assert_eq!(vt.cursor_style(), CursorStyle::Underline);
        vt.feed(b"\x1b[?25l");
        assert!(vt.render().cursor.is_none());
        vt.feed(b"\x1b[?25h");
        assert!(vt.render().cursor.is_some());
    }

    #[test]
    fn an_unfocused_terminal_outlines_its_cursor() {
        let mut vt = vt(10, 3);
        vt.screen_mut().set_focused(false);
        assert_eq!(
            vt.render().cursor.expect("cursor").style,
            CursorStyle::HollowBlock
        );
    }

    #[test]
    fn the_cursor_leaves_the_frame_when_the_view_scrolls_past_it() {
        let mut vt = vt(10, 3);
        for i in 0..10 {
            vt.feed(format!("line{i}\r\n").as_bytes());
        }
        assert!(vt.render().cursor.is_some());
        vt.screen_mut().scroll_viewport(-5);
        assert!(vt.render().cursor.is_none());
    }

    #[test]
    fn a_row_collapses_into_runs_of_one_style_each() {
        let mut vt = vt(20, 2);
        vt.feed(b"aa\x1b[1mbb\x1b[0mcc");
        let rows = vt.render().rows.clone();
        let runs = &rows[0].runs;
        assert_eq!(runs.len(), 3);
        assert_eq!(runs[0].text, "aa");
        assert_eq!(runs[1].text, "bb");
        assert_eq!(runs[2].text, "cc");
        // The blank tail of the row is nothing to paint.
        assert!(runs.iter().all(|run| !run.text.ends_with(' ')));
    }

    #[test]
    fn a_coloured_blank_tail_is_kept_because_it_is_visible() {
        let mut vt = vt(6, 2);
        vt.feed(b"\x1b[41m\x1b[K");
        let rows = vt.render().rows.clone();
        assert_eq!(rows[0].runs.len(), 1);
        assert_eq!(rows[0].runs[0].text, "      ");
        assert_eq!(rows[0].runs[0].bg, DEFAULT_ANSI[1]);
    }

    #[test]
    fn an_unchanged_frame_is_not_rebuilt() {
        let mut vt = vt(10, 3);
        vt.feed(b"hi");
        assert!(vt.render().dirty);
        assert!(!vt.render().dirty);
        vt.feed(b"!");
        assert!(vt.render().dirty);
    }

    #[test]
    fn a_theme_change_repaints_every_row() {
        let mut vt = vt(10, 2);
        vt.feed(b"\x1b[31mred");
        let before = vt.render().rows[0].runs[0].fg;
        vt.screen_mut().set_theme(&LIGHT);
        let state = vt.render();
        assert!(state.dirty);
        assert_ne!(state.rows[0].runs[0].fg, before);
        assert_eq!(state.background, LIGHT.background);
    }

    #[test]
    fn selection_spans_rows_and_joins_a_wrapped_line() {
        let mut vt = vt(5, 4);
        vt.feed(b"abcdefgh\r\nnext");
        vt.screen_mut()
            .selection_start(0, 0, SelectionMode::Character);
        vt.screen_mut().selection_extend(2, 1);
        // The wrap was the terminal's doing, not the user's: no newline.
        assert_eq!(vt.selection_text(), "abcdefgh");

        vt.screen_mut().selection_extend(3, 2);
        assert_eq!(vt.selection_text(), "abcdefgh\nnext");
    }

    #[test]
    fn a_double_click_takes_the_whole_word() {
        let mut vt = vt(30, 2);
        vt.feed(b"run ./build.sh now");
        vt.screen_mut().selection_start(6, 0, SelectionMode::Word);
        assert_eq!(vt.selection_text(), "./build.sh");
    }

    #[test]
    fn a_triple_click_takes_the_whole_line() {
        let mut vt = vt(30, 3);
        vt.feed(b"first line\r\nsecond line");
        vt.screen_mut().selection_start(3, 1, SelectionMode::Line);
        assert_eq!(vt.selection_text(), "second line");
    }

    #[test]
    fn a_selection_shows_up_as_a_column_range_on_the_frame() {
        let mut vt = vt(10, 3);
        vt.feed(b"hello");
        vt.screen_mut()
            .selection_start(1, 0, SelectionMode::Character);
        vt.screen_mut().selection_extend(3, 0);
        let rows = vt.render().rows.clone();
        assert_eq!(rows[0].selection, Some((1, 3)));
        assert_eq!(rows[1].selection, None);
    }

    #[test]
    fn a_selection_stays_on_its_text_as_output_scrolls_past() {
        let mut vt = vt(10, 3);
        vt.feed(b"target\r\n");
        vt.screen_mut().selection_start(0, 0, SelectionMode::Line);
        for i in 0..5 {
            vt.feed(format!("noise{i}\r\n").as_bytes());
        }
        assert_eq!(vt.selection_text(), "target");
    }

    #[test]
    fn insert_and_delete_move_the_rest_of_the_line() {
        let mut vt = vt(10, 2);
        vt.feed(b"abcdef\x1b[1;3H\x1b[2@");
        assert_eq!(vt.row_text(0), "ab  cdef");
        vt.feed(b"\x1b[2P");
        assert_eq!(vt.row_text(0), "abcdef");
        vt.feed(b"\x1b[1;3H\x1b[2X");
        assert_eq!(vt.row_text(0), "ab  ef");
    }

    #[test]
    fn insert_and_delete_line_shuffle_rows_inside_the_region() {
        let mut vt = vt(10, 4);
        vt.feed(b"a\r\nb\r\nc\r\nd");
        vt.feed(b"\x1b[2;1H\x1b[L");
        assert_eq!(row_texts(&vt, 4), vec!["a", "", "b", "c"]);
        vt.feed(b"\x1b[2;1H\x1b[M");
        assert_eq!(row_texts(&vt, 4), vec!["a", "b", "c", ""]);
    }

    #[test]
    fn save_and_restore_bring_back_the_cursor_and_its_pen() {
        let mut vt = vt(20, 3);
        vt.feed(b"\x1b[2;5H\x1b[1;31m\x1b7");
        vt.feed(b"\x1b[1;1H\x1b[0mplain\x1b8X");
        assert_eq!(vt.cursor(), (5, 1));
        assert_eq!(vt.row_text(1), "    X");
        // The pen came back with the cursor, so the X is red and bold rather
        // than sharing the blanks' default style.
        let rows = vt.render().rows.clone();
        let last = rows[1].runs.last().expect("the X is painted");
        assert_eq!(last.text, "X");
        assert!(last.bold);
        assert_eq!(last.fg, vt.screen().palette()[1]);
    }

    #[test]
    fn a_resize_keeps_the_cursor_inside_the_grid() {
        let mut vt = vt(20, 6);
        vt.feed(b"\x1b[6;20Hx");
        vt.screen_mut().resize(10, 3);
        let (x, y) = vt.cursor();
        assert!(x < 10 && y < 3);
        assert_eq!(vt.cols(), 10);
        assert_eq!(vt.rows(), 3);
        assert_eq!(vt.render().rows.len(), 3);
    }

    #[test]
    fn shrinking_the_grid_pushes_the_top_into_history() {
        let mut vt = vt(10, 4);
        vt.feed(b"one\r\ntwo\r\nthree\r\nfour");
        vt.screen_mut().resize(10, 2);
        assert_eq!(vt.scrollback_len(), 2);
        assert_eq!(row_texts(&vt, 2), vec!["three", "four"]);
    }

    #[test]
    fn a_full_reset_puts_every_mode_back() {
        let mut vt = vt(10, 3);
        vt.feed(b"\x1b[?2004h\x1b[?1049h\x1b[1;31mtext\x1bc");
        assert!(!vt.bracketed_paste());
        assert!(!vt.on_alt_screen());
        assert_eq!(vt.cursor(), (0, 0));
        assert_eq!(vt.viewport_text().trim(), "");
    }

    #[test]
    fn utf8_arriving_split_across_two_reads_still_prints_one_character() {
        let mut vt = vt(10, 2);
        vt.feed(&[0xE2, 0x80]);
        vt.feed(&[0x94]);
        assert_eq!(vt.row_text(0), "—");
    }
}
