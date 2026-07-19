"""A terminal emulator widget powered by libghostty-vt.

libghostty (vendor/ghostty) provides the terminal core: VT parsing, screen
state, render-state extraction, and key encoding. This widget provides the
platform half: a PTY running the user's shell, Qt painting of the cell grid,
and translation of Qt input events into ghostty key events.
"""

from __future__ import annotations

import ctypes
import fcntl
import os
import shutil
import signal
import struct
import termios
from typing import TYPE_CHECKING

from PySide6.QtCore import QSocketNotifier, Qt, QTimer
from PySide6.QtGui import (
    QColor,
    QFont,
    QFontDatabase,
    QFontMetricsF,
    QGuiApplication,
    QPainter,
)
from PySide6.QtWidgets import QWidget

from kraken.terminal import ghostty_vt as g
from kraken.terminal.keymap import QT_KEY_MAP, UNSHIFTED
from kraken.ui.themes import DEFAULT_THEME, THEMES

if TYPE_CHECKING:
    from kraken.agent.remote import RemoteTarget

# Cell flag bits for the row model.
_BOLD = 1
_ITALIC = 2
_UNDERLINE = 4
_STRIKE = 8
_INVERSE = 16
_FAINT = 32
_INVISIBLE = 64

# Lines of off-screen history each terminal keeps. libghostty holds this in
# C-side memory per terminal (cols × cells per line), so it's a direct
# per-terminal memory cost; 5k lines is plenty of scrollback while roughly
# halving that buffer versus the old 10k.
_MAX_SCROLLBACK = 5_000


def _check(result: int, what: str) -> None:
    if result != g.SUCCESS:
        raise RuntimeError(f"libghostty-vt: {what} failed with {result}")


def _pick_term() -> str:
    for d in ("/usr/share/terminfo/x", os.path.expanduser("~/.terminfo/x")):
        if os.path.exists(os.path.join(d, "xterm-ghostty")):
            return "xterm-ghostty"
    return "xterm-256color"


def _spawn_session(program: str, argv: list[str], env: dict, slave_path: str) -> int:
    """Start `program` (with `argv`) in a new session with `slave_path` as its
    controlling terminal, returning the child pid. For a local terminal this is
    the user's shell; for a remote workspace it is `ssh` opening a login shell
    on the far host.

    The fast path is posix_spawn(setsid=True). Some CPython builds — notably
    the relocatable python-build-standalone interpreter shipped inside the
    AppImage — are compiled against a glibc too old for POSIX_SPAWN_SETSID, so
    that call raises NotImplementedError regardless of the runtime glibc. Fall
    back to an equivalent manual fork/setsid/exec.
    """
    try:
        return os.posix_spawn(
            program,
            argv,
            env,
            file_actions=[
                (os.POSIX_SPAWN_OPEN, 0, slave_path, os.O_RDWR, 0),
                (os.POSIX_SPAWN_DUP2, 0, 1),
                (os.POSIX_SPAWN_DUP2, 0, 2),
            ],
            setsid=True,
        )
    except NotImplementedError:
        pass

    pid = os.fork()
    if pid != 0:
        return pid
    # --- child: give ourselves a new session, then adopt the pty slave as the
    # controlling terminal by opening it (without O_NOCTTY) as fds 0/1/2.
    try:
        os.setsid()
        fd = os.open(slave_path, os.O_RDWR)
        os.dup2(fd, 0)
        os.dup2(fd, 1)
        os.dup2(fd, 2)
        if fd > 2:
            os.close(fd)
        os.execve(program, argv, env)
    except BaseException:
        os._exit(127)


class GhosttyTerminalWidget(QWidget):
    """Interactive shell terminal rendered from libghostty-vt render state."""

    def __init__(
        self,
        parent: QWidget | None = None,
        cwd: str | None = None,
        remote: "RemoteTarget | None" = None,
    ):
        super().__init__(parent)
        self._cwd = cwd
        # When set, the terminal is an ssh login shell on the remote host
        # instead of a local shell.
        self._remote = remote
        self.setFocusPolicy(Qt.FocusPolicy.StrongFocus)
        self.setAttribute(Qt.WidgetAttribute.WA_OpaquePaintEvent)
        self.setAttribute(Qt.WidgetAttribute.WA_InputMethodEnabled)
        self.setCursor(Qt.CursorShape.IBeamCursor)

        font = QFontDatabase.systemFont(QFontDatabase.SystemFont.FixedFont)
        font.setPointSizeF(10.5)
        self.setFont(font)
        self._update_font_metrics()

        self._cols = 80
        self._rows_count = 24

        # libghostty handles
        self._term = ctypes.c_void_p()
        opts = g.TerminalOptions(
            cols=self._cols, rows=self._rows_count, max_scrollback=_MAX_SCROLLBACK
        )
        _check(g.lib.ghostty_terminal_new(None, ctypes.byref(self._term), opts), "terminal_new")

        self._theme = THEMES[DEFAULT_THEME]
        self._apply_theme_colors()

        # The write_pty callback must outlive the terminal; keep a reference.
        self._write_pty_cb = g.WritePtyFn(self._on_write_pty)
        g.lib.ghostty_terminal_set(self._term, g.TERMINAL_OPT_WRITE_PTY, self._write_pty_cb)

        self._render_state = ctypes.c_void_p()
        _check(g.lib.ghostty_render_state_new(None, ctypes.byref(self._render_state)), "render_state_new")
        self._row_iter = ctypes.c_void_p()
        _check(g.lib.ghostty_render_state_row_iterator_new(None, ctypes.byref(self._row_iter)), "row_iterator_new")
        self._cells = ctypes.c_void_p()
        _check(g.lib.ghostty_render_state_row_cells_new(None, ctypes.byref(self._cells)), "row_cells_new")

        self._key_encoder = ctypes.c_void_p()
        _check(g.lib.ghostty_key_encoder_new(None, ctypes.byref(self._key_encoder)), "key_encoder_new")
        self._key_event = ctypes.c_void_p()
        _check(g.lib.ghostty_key_event_new(None, ctypes.byref(self._key_event)), "key_event_new")

        # Screen model rebuilt from render state: one list of cell tuples
        # (char|None, fg|None, bg|None, flags) per visible row.
        self._row_model: list[list[tuple]] = []
        # Per-row selected column range (start_x, end_x) or None.
        self._row_selection: list[tuple[int, int] | None] = []
        self._colors = g.RenderStateColors(size=ctypes.sizeof(g.RenderStateColors))
        self._cursor: tuple[int, int, int] | None = None
        self._child_exited = False

        # Selection gesture engine (click/drag/word/line semantics live in
        # libghostty; we only feed it grid refs and pixel positions).
        self._gesture = ctypes.c_void_p()
        _check(g.lib.ghostty_selection_gesture_new(None, ctypes.byref(self._gesture)), "gesture_new")
        self._gesture_events = {}
        for ev_type in (
            g.GESTURE_EVENT_PRESS,
            g.GESTURE_EVENT_RELEASE,
            g.GESTURE_EVENT_DRAG,
            g.GESTURE_EVENT_AUTOSCROLL_TICK,
        ):
            handle = ctypes.c_void_p()
            _check(
                g.lib.ghostty_selection_gesture_event_new(None, ctypes.byref(handle), ev_type),
                "gesture_event_new",
            )
            self._gesture_events[ev_type] = handle
        self._selecting = False
        self._last_mouse_pos = None
        self._autoscroll_timer = QTimer(self)
        self._autoscroll_timer.setInterval(50)
        self._autoscroll_timer.timeout.connect(self._autoscroll_tick)

        self._render_timer = QTimer(self)
        self._render_timer.setSingleShot(True)
        self._render_timer.setInterval(10)
        self._render_timer.timeout.connect(self._sync_render_state)

        # Resizes are debounced: applying every intermediate size during a
        # splitter drag makes the shell redraw its prompt dozens of times,
        # and the leftovers reflow into garbled lines. Worse, two quick
        # PTY resizes can coalesce into one SIGWINCH that the shell sees as
        # "no net change" and skips redrawing, while the reflow of the
        # intermediate size already moved the real cursor — so keep the
        # debounce long enough that mid-drag pauses don't apply.
        self._force_full_rebuild = False
        self._resize_timer = QTimer(self)
        self._resize_timer.setSingleShot(True)
        self._resize_timer.setInterval(150)
        self._resize_timer.timeout.connect(self._apply_resize)

        self._spawn_shell()
        self._sync_render_state()

    # ---- PTY ----------------------------------------------------------

    def _spawn_shell(self) -> None:
        self._master_fd, slave_fd = os.openpty()
        slave_path = os.ttyname(slave_fd)

        env = dict(os.environ)
        env["COLORTERM"] = "truecolor"

        if self._remote is not None:
            # An ssh client on the local pty, opening an interactive login
            # shell on the remote host in the workspace path. The remote host
            # is unlikely to have ghostty's terminfo, so advertise a widely
            # available terminal; ssh forwards TERM to the remote pty.
            env["TERM"] = "xterm-256color"
            argv = self._remote.terminal_argv()
            program = shutil.which("ssh") or "/usr/bin/ssh"
            local_cwd = None
        else:
            program = os.environ.get("SHELL", "/bin/bash")
            argv = [program]
            env["TERM"] = _pick_term()
            local_cwd = self._cwd

        # The child opens the pty slave as fd 0 after setsid(), which makes it
        # the controlling terminal. Neither spawn path has a cwd parameter, so
        # chdir around it (local shells only); Qt runs single-threaded here so
        # nothing else observes the switch.
        old_cwd = os.getcwd() if local_cwd else None
        if local_cwd:
            os.chdir(local_cwd)
        try:
            self._child_pid = _spawn_session(program, argv, env, slave_path)
        finally:
            if old_cwd is not None:
                os.chdir(old_cwd)
        os.close(slave_fd)
        os.set_blocking(self._master_fd, False)

        self._notifier = QSocketNotifier(self._master_fd, QSocketNotifier.Type.Read, self)
        self._notifier.activated.connect(self._on_pty_readable)

    def _on_pty_readable(self) -> None:
        total = 0
        while total < 1 << 20:
            try:
                chunk = os.read(self._master_fd, 65536)
            except BlockingIOError:
                break
            except OSError:
                chunk = b""
            if not chunk:
                self._on_child_exit()
                return
            arr = (ctypes.c_uint8 * len(chunk)).from_buffer_copy(chunk)
            g.lib.ghostty_terminal_vt_write(self._term, arr, len(chunk))
            total += len(chunk)
        if not self._render_timer.isActive():
            self._render_timer.start()

    def _on_write_pty(self, _term, _userdata, data, length) -> None:
        # Terminal responses (DSR, DECRQM, ...) go back to the shell.
        if length and not self._child_exited:
            try:
                os.write(self._master_fd, ctypes.string_at(data, length))
            except OSError:
                pass

    def _write_input(self, data: bytes) -> None:
        if self._child_exited or not data:
            return
        self._scroll(g.SCROLL_VIEWPORT_BOTTOM)
        try:
            os.write(self._master_fd, data)
        except OSError:
            pass

    def _on_child_exit(self) -> None:
        if self._child_exited:
            return
        self._child_exited = True
        self._notifier.setEnabled(False)
        try:
            os.waitpid(self._child_pid, os.WNOHANG)
        except ChildProcessError:
            pass
        msg = b"\r\n\x1b[90m[process exited]\x1b[0m\r\n"
        arr = (ctypes.c_uint8 * len(msg)).from_buffer_copy(msg)
        g.lib.ghostty_terminal_vt_write(self._term, arr, len(msg))
        self._sync_render_state()

    def shutdown(self) -> None:
        """Terminate the shell and free libghostty resources."""
        if not self._child_exited:
            self._notifier.setEnabled(False)
            try:
                os.kill(self._child_pid, signal.SIGHUP)
                os.waitpid(self._child_pid, os.WNOHANG)
            except (ProcessLookupError, ChildProcessError):
                pass
            try:
                os.close(self._master_fd)
            except OSError:
                pass
            self._child_exited = True
        if self._term:
            for handle in self._gesture_events.values():
                g.lib.ghostty_selection_gesture_event_free(handle)
            g.lib.ghostty_selection_gesture_free(self._gesture, self._term)
            g.lib.ghostty_render_state_row_cells_free(self._cells)
            g.lib.ghostty_render_state_row_iterator_free(self._row_iter)
            g.lib.ghostty_render_state_free(self._render_state)
            g.lib.ghostty_key_event_free(self._key_event)
            g.lib.ghostty_key_encoder_free(self._key_encoder)
            g.lib.ghostty_terminal_free(self._term)
            self._term = ctypes.c_void_p()

    # ---- Theme ---------------------------------------------------------

    @property
    def theme_name(self) -> str:
        return self._theme.name

    def set_theme(self, name: str) -> None:
        """Switch to a named theme (see app.themes.THEMES) and repaint."""
        if not self._term:
            return
        self._theme = THEMES[name]
        self._apply_theme_colors()
        # Cell colors are resolved against the palette when rows are read,
        # so every row must be re-read even though none are dirty.
        self._force_full_rebuild = True
        self._sync_render_state()

    def _apply_theme_colors(self) -> None:
        theme = self._theme
        bg = g.ColorRgb(*theme.background)
        fg = g.ColorRgb(*theme.foreground)
        g.lib.ghostty_terminal_set(self._term, g.TERMINAL_OPT_COLOR_BACKGROUND, ctypes.byref(bg))
        g.lib.ghostty_terminal_set(self._term, g.TERMINAL_OPT_COLOR_FOREGROUND, ctypes.byref(fg))
        palette = theme.palette256()
        if palette is None:
            # NULL resets to libghostty's built-in default palette.
            g.lib.ghostty_terminal_set(self._term, g.TERMINAL_OPT_COLOR_PALETTE, None)
        else:
            arr = (g.ColorRgb * 256)(*(g.ColorRgb(*c) for c in palette))
            g.lib.ghostty_terminal_set(self._term, g.TERMINAL_OPT_COLOR_PALETTE, ctypes.byref(arr))

    # ---- Render state -> row model ------------------------------------

    def _sync_render_state(self) -> None:
        if not self._term:
            return
        _check(g.lib.ghostty_render_state_update(self._render_state, self._term), "render_state_update")

        dirty = ctypes.c_int()
        g.lib.ghostty_render_state_get(self._render_state, g.RENDER_STATE_DATA_DIRTY, ctypes.byref(dirty))
        rows = ctypes.c_uint16()
        g.lib.ghostty_render_state_get(self._render_state, g.RENDER_STATE_DATA_ROWS, ctypes.byref(rows))
        full = (
            dirty.value == g.RENDER_STATE_DIRTY_FULL
            or len(self._row_model) != rows.value
            or self._force_full_rebuild
        )
        self._force_full_rebuild = False

        g.lib.ghostty_render_state_colors_get(self._render_state, ctypes.byref(self._colors))

        if full:
            self._row_model = [[] for _ in range(rows.value)]
        self._row_selection = [None] * rows.value

        _check(
            g.lib.ghostty_render_state_get(
                self._render_state, g.RENDER_STATE_DATA_ROW_ITERATOR, ctypes.byref(self._row_iter)
            ),
            "get row_iterator",
        )

        clean = ctypes.c_bool(False)
        row_sel = g.RowSelection(size=ctypes.sizeof(g.RowSelection))
        y = 0
        while g.lib.ghostty_render_state_row_iterator_next(self._row_iter):
            if y >= len(self._row_model):
                break
            row_dirty = ctypes.c_bool(False)
            g.lib.ghostty_render_state_row_get(self._row_iter, g.ROW_DATA_DIRTY, ctypes.byref(row_dirty))
            if full or row_dirty.value:
                self._row_model[y] = self._read_row_cells()
                g.lib.ghostty_render_state_row_set(self._row_iter, g.ROW_OPTION_DIRTY, ctypes.byref(clean))
            # Selection is refreshed for every row on every sync; selection
            # changes do not necessarily mark rows dirty.
            if (
                g.lib.ghostty_render_state_row_get(
                    self._row_iter, g.ROW_DATA_SELECTION, ctypes.byref(row_sel)
                )
                == g.SUCCESS
            ):
                self._row_selection[y] = (row_sel.start_x, row_sel.end_x)
            y += 1

        not_dirty = ctypes.c_int(g.RENDER_STATE_DIRTY_FALSE)
        g.lib.ghostty_render_state_set(self._render_state, g.RENDER_STATE_OPTION_DIRTY, ctypes.byref(not_dirty))

        self._read_cursor()
        self.update()

    def _read_row_cells(self) -> list[tuple]:
        _check(
            g.lib.ghostty_render_state_row_get(self._row_iter, g.ROW_DATA_CELLS, ctypes.byref(self._cells)),
            "get row cells",
        )
        cells = self._cells
        out = []
        utf8_buf = ctypes.create_string_buffer(64)
        buf = g.Buffer(ptr=ctypes.cast(utf8_buf, ctypes.c_void_p), cap=64, len=0)
        glen = ctypes.c_uint32()
        color = g.ColorRgb()
        styled = ctypes.c_bool()
        style = g.Style(size=ctypes.sizeof(g.Style))

        while g.lib.ghostty_render_state_row_cells_next(cells):
            bg = None
            if g.lib.ghostty_render_state_row_cells_get(cells, g.CELLS_DATA_BG_COLOR, ctypes.byref(color)) == g.SUCCESS:
                bg = (color.r, color.g, color.b)

            g.lib.ghostty_render_state_row_cells_get(cells, g.CELLS_DATA_GRAPHEMES_LEN, ctypes.byref(glen))
            if glen.value == 0:
                out.append((None, None, bg, 0))
                continue

            buf.len = 0
            ch = None
            if (
                g.lib.ghostty_render_state_row_cells_get(cells, g.CELLS_DATA_GRAPHEMES_UTF8, ctypes.byref(buf))
                == g.SUCCESS
                and buf.len
            ):
                ch = utf8_buf.raw[: buf.len].decode("utf-8", "replace")

            fg = None
            if g.lib.ghostty_render_state_row_cells_get(cells, g.CELLS_DATA_FG_COLOR, ctypes.byref(color)) == g.SUCCESS:
                fg = (color.r, color.g, color.b)

            flags = 0
            g.lib.ghostty_render_state_row_cells_get(cells, g.CELLS_DATA_HAS_STYLING, ctypes.byref(styled))
            if styled.value:
                g.lib.ghostty_render_state_row_cells_get(cells, g.CELLS_DATA_STYLE, ctypes.byref(style))
                if style.bold:
                    flags |= _BOLD
                if style.italic:
                    flags |= _ITALIC
                if style.underline:
                    flags |= _UNDERLINE
                if style.strikethrough:
                    flags |= _STRIKE
                if style.inverse:
                    flags |= _INVERSE
                if style.faint:
                    flags |= _FAINT
                if style.invisible:
                    flags |= _INVISIBLE
            out.append((ch, fg, bg, flags))
        return out

    def _read_cursor(self) -> None:
        visible = ctypes.c_bool()
        in_viewport = ctypes.c_bool()
        g.lib.ghostty_render_state_get(self._render_state, g.RENDER_STATE_DATA_CURSOR_VISIBLE, ctypes.byref(visible))
        g.lib.ghostty_render_state_get(
            self._render_state, g.RENDER_STATE_DATA_CURSOR_VIEWPORT_HAS_VALUE, ctypes.byref(in_viewport)
        )
        if not (visible.value and in_viewport.value):
            self._cursor = None
            return
        cx = ctypes.c_uint16()
        cy = ctypes.c_uint16()
        cstyle = ctypes.c_int()
        g.lib.ghostty_render_state_get(self._render_state, g.RENDER_STATE_DATA_CURSOR_VIEWPORT_X, ctypes.byref(cx))
        g.lib.ghostty_render_state_get(self._render_state, g.RENDER_STATE_DATA_CURSOR_VIEWPORT_Y, ctypes.byref(cy))
        g.lib.ghostty_render_state_get(
            self._render_state, g.RENDER_STATE_DATA_CURSOR_VISUAL_STYLE, ctypes.byref(cstyle)
        )
        self._cursor = (cx.value, cy.value, cstyle.value)

    # ---- Painting ------------------------------------------------------

    def _update_font_metrics(self) -> None:
        fm = QFontMetricsF(self.font())
        self._cell_w = max(1, round(fm.horizontalAdvance("M")))
        self._cell_h = max(1, round(fm.height()))
        self._ascent = round(fm.ascent())

    def paintEvent(self, event) -> None:
        painter = QPainter(self)
        default_bg = QColor(*self._theme.background)
        default_fg = QColor(*self._theme.foreground)
        c = self._colors
        if c.size:
            default_bg = QColor(c.background.r, c.background.g, c.background.b)
            default_fg = QColor(c.foreground.r, c.foreground.g, c.foreground.b)
        painter.fillRect(self.rect(), default_bg)

        cw, chh = self._cell_w, self._cell_h
        base_font = self.font()
        painter.setFont(base_font)
        current_variant = (False, False, False, False)

        for y, row in enumerate(self._row_model):
            py = y * chh
            # Background pass: coalesce runs of identical background.
            run_start = None
            run_bg = None
            for x in range(len(row) + 1):
                cell_bg = None
                if x < len(row):
                    ch, fg, bg, flags = row[x]
                    cell_bg = bg
                    if flags & _INVERSE:
                        cell_bg = fg if fg is not None else (
                            default_fg.red(), default_fg.green(), default_fg.blue()
                        )
                if cell_bg != run_bg:
                    if run_bg is not None and run_start is not None:
                        painter.fillRect(
                            run_start * cw, py, (x - run_start) * cw, chh, QColor(*run_bg)
                        )
                    run_start = x
                    run_bg = cell_bg

            # Selection highlight: above cell backgrounds, below text.
            if y < len(self._row_selection) and self._row_selection[y] is not None:
                sel_start, sel_end = self._row_selection[y]
                painter.fillRect(
                    sel_start * cw,
                    py,
                    (sel_end - sel_start + 1) * cw,
                    chh,
                    QColor(120, 150, 210, 110),
                )

            # Text pass.
            for x, (ch, fg, bg, flags) in enumerate(row):
                if ch is None or ch == " " or flags & _INVISIBLE:
                    continue
                eff_fg = fg
                if flags & _INVERSE:
                    eff_fg = bg if bg is not None else (
                        default_bg.red(), default_bg.green(), default_bg.blue()
                    )
                color = QColor(*eff_fg) if eff_fg is not None else default_fg
                if flags & _FAINT:
                    color = QColor(color)
                    color.setAlpha(160)
                variant = (
                    bool(flags & _BOLD),
                    bool(flags & _ITALIC),
                    bool(flags & _UNDERLINE),
                    bool(flags & _STRIKE),
                )
                if variant != current_variant:
                    font = QFont(base_font)
                    font.setBold(variant[0])
                    font.setItalic(variant[1])
                    font.setUnderline(variant[2])
                    font.setStrikeOut(variant[3])
                    painter.setFont(font)
                    current_variant = variant
                painter.setPen(color)
                painter.drawText(x * cw, py + self._ascent, ch)

        self._paint_cursor(painter, default_fg, default_bg)

    def _paint_cursor(self, painter: QPainter, default_fg: QColor, default_bg: QColor) -> None:
        if self._cursor is None:
            return
        x, y, style = self._cursor
        cw, chh = self._cell_w, self._cell_h
        px, py = x * cw, y * chh
        color = default_fg
        c = self._colors
        if c.size and c.cursor_has_value:
            color = QColor(c.cursor.r, c.cursor.g, c.cursor.b)

        if not self.hasFocus():
            style = g.CURSOR_STYLE_BLOCK_HOLLOW

        if style == g.CURSOR_STYLE_BLOCK:
            painter.fillRect(px, py, cw, chh, color)
            if y < len(self._row_model) and x < len(self._row_model[y]):
                ch = self._row_model[y][x][0]
                if ch and ch != " ":
                    painter.setPen(default_bg)
                    painter.setFont(self.font())
                    painter.drawText(px, py + self._ascent, ch)
        elif style == g.CURSOR_STYLE_BAR:
            painter.fillRect(px, py, 2, chh, color)
        elif style == g.CURSOR_STYLE_UNDERLINE:
            painter.fillRect(px, py + chh - 2, cw, 2, color)
        else:  # hollow block
            painter.setPen(color)
            painter.drawRect(px, py, cw - 1, chh - 1)

    # ---- Geometry ------------------------------------------------------

    def sizeHint(self):
        from PySide6.QtCore import QSize

        return QSize(80 * self._cell_w, 24 * self._cell_h)

    def resizeEvent(self, event) -> None:
        super().resizeEvent(event)
        cols = max(4, self.width() // self._cell_w)
        rows = max(2, self.height() // self._cell_h)
        if cols == self._cols and rows == self._rows_count:
            self._resize_timer.stop()
            return
        self._resize_timer.start()

    def showEvent(self, event) -> None:
        # Coming back from a hidden stack page: apply any pending size now,
        # so the shell gets one clean SIGWINCH before the user can type and
        # the first paint already uses the right grid.
        super().showEvent(event)
        self._resize_timer.stop()
        self._apply_resize()

    def _apply_resize(self) -> None:
        # Never resize the PTY while hidden (e.g. in a background workspace):
        # the shell would redraw unseen, and repeated hidden resizes can
        # desync its cursor bookkeeping. showEvent applies the final size.
        if not self.isVisible() or self.width() <= 0 or self.height() <= 0:
            return
        cols = max(4, self.width() // self._cell_w)
        rows = max(2, self.height() // self._cell_h)
        if cols == self._cols and rows == self._rows_count:
            return
        self._cols, self._rows_count = cols, rows
        g.lib.ghostty_terminal_resize(
            self._term, cols, rows, cols * self._cell_w, rows * self._cell_h
        )
        if not self._child_exited:
            winsz = struct.pack(
                "HHHH", rows, cols, cols * self._cell_w, rows * self._cell_h
            )
            try:
                fcntl.ioctl(self._master_fd, termios.TIOCSWINSZ, winsz)
            except OSError:
                pass
        self._force_full_rebuild = True
        if not self._render_timer.isActive():
            self._render_timer.start()

    # ---- Input ---------------------------------------------------------

    def event(self, ev):
        # Keep Tab/Backtab for the shell instead of Qt focus traversal.
        from PySide6.QtCore import QEvent

        if ev.type() == QEvent.Type.KeyPress and ev.key() in (
            Qt.Key.Key_Tab,
            Qt.Key.Key_Backtab,
        ):
            self.keyPressEvent(ev)
            return True
        return super().event(ev)

    def _qt_mods_to_ghostty(self, qt_mods) -> int:
        mods = 0
        if qt_mods & Qt.KeyboardModifier.ShiftModifier:
            mods |= g.MODS_SHIFT
        if qt_mods & Qt.KeyboardModifier.ControlModifier:
            mods |= g.MODS_CTRL
        if qt_mods & Qt.KeyboardModifier.AltModifier:
            mods |= g.MODS_ALT
        if qt_mods & Qt.KeyboardModifier.MetaModifier:
            mods |= g.MODS_SUPER
        return mods

    def _encode_key(self, ev, action: int) -> bytes:
        key_name = QT_KEY_MAP.get(Qt.Key(ev.key()), "UNIDENTIFIED")
        mods = self._qt_mods_to_ghostty(ev.modifiers())

        text = ev.text()
        printable = text and all(ord(ch) >= 0x20 and ord(ch) != 0x7F for ch in text)

        kev = self._key_event
        g.lib.ghostty_key_event_set_action(kev, action)
        g.lib.ghostty_key_event_set_key(kev, g.KEY[key_name])
        g.lib.ghostty_key_event_set_mods(kev, mods)

        consumed = 0
        if printable and mods & g.MODS_SHIFT:
            consumed = g.MODS_SHIFT
        g.lib.ghostty_key_event_set_consumed_mods(kev, consumed)

        utf8 = text.encode() if printable else b""
        g.lib.ghostty_key_event_set_utf8(kev, utf8 or None, len(utf8))

        unshifted = UNSHIFTED.get(key_name, 0)
        if printable and len(text) == 1 and not unshifted:
            unshifted = ord(text.lower())
        g.lib.ghostty_key_event_set_unshifted_codepoint(kev, unshifted)

        # Sync encoder options (kitty flags, application cursor keys, ...)
        # from live terminal state, then encode.
        g.lib.ghostty_key_encoder_setopt_from_terminal(self._key_encoder, self._term)
        buf = ctypes.create_string_buffer(128)
        written = ctypes.c_size_t()
        result = g.lib.ghostty_key_encoder_encode(
            self._key_encoder, kev, buf, len(buf), ctypes.byref(written)
        )
        if result != g.SUCCESS:
            return b""
        return buf.raw[: written.value]

    def keyPressEvent(self, ev) -> None:
        # Ctrl+Shift+C / Ctrl+Shift+V copy/paste, matching ghostty's
        # default Linux bindings.
        ctrl_shift = Qt.KeyboardModifier.ControlModifier | Qt.KeyboardModifier.ShiftModifier
        if ev.modifiers() & ctrl_shift == ctrl_shift:
            if ev.key() == Qt.Key.Key_V:
                self._paste(QGuiApplication.clipboard().text())
                return
            if ev.key() == Qt.Key.Key_C:
                self.copy_selection()
                return
        action = g.KEY_ACTION_REPEAT if ev.isAutoRepeat() else g.KEY_ACTION_PRESS
        data = self._encode_key(ev, action)
        if data:
            self._clear_selection()
            self._write_input(data)
        ev.accept()

    def keyReleaseEvent(self, ev) -> None:
        if ev.isAutoRepeat():
            return
        # Only encodes anything when the kitty keyboard protocol asks for it.
        data = self._encode_key(ev, g.KEY_ACTION_RELEASE)
        if data:
            self._write_input(data)
        ev.accept()

    def inputMethodEvent(self, ev) -> None:
        text = ev.commitString()
        if text:
            self._write_input(text.encode())
        ev.accept()

    def _paste(self, text: str) -> None:
        if not text:
            return
        bracketed = ctypes.c_bool(False)
        g.lib.ghostty_terminal_mode_get(
            self._term, g.MODE_BRACKETED_PASTE, ctypes.byref(bracketed)
        )
        raw = text.encode()
        data = ctypes.create_string_buffer(raw, len(raw))
        out = ctypes.create_string_buffer(len(raw) + 16)
        written = ctypes.c_size_t()
        result = g.lib.ghostty_paste_encode(
            data, len(raw), bracketed.value, out, len(out), ctypes.byref(written)
        )
        if result == g.SUCCESS:
            self._write_input(out.raw[: written.value])

    # ---- Selection -----------------------------------------------------

    def _grid_ref_at(self, pos) -> g.GridRef | None:
        x = min(max(int(pos.x()) // self._cell_w, 0), self._cols - 1)
        y = min(max(int(pos.y()) // self._cell_h, 0), self._rows_count - 1)
        point = g.Point(tag=g.POINT_TAG_VIEWPORT)
        point.value.coordinate = g.PointCoordinate(x=x, y=y)
        ref = g.GridRef(size=ctypes.sizeof(g.GridRef))
        if g.lib.ghostty_terminal_grid_ref(self._term, point, ctypes.byref(ref)) != g.SUCCESS:
            return None
        return ref

    def _fire_gesture(self, ev_type: int, opts: list[tuple[int, object]]) -> bool:
        """Send a gesture event; apply and render any produced selection."""
        handle = self._gesture_events[ev_type]
        for opt, value in opts:
            g.lib.ghostty_selection_gesture_event_set(
                handle, opt, ctypes.byref(value) if value is not None else None
            )
        selection = g.Selection(size=ctypes.sizeof(g.Selection))
        result = g.lib.ghostty_selection_gesture_event(
            self._gesture, self._term, handle, ctypes.byref(selection)
        )
        if result == g.SUCCESS:
            g.lib.ghostty_terminal_set(self._term, g.TERMINAL_OPT_SELECTION, ctypes.byref(selection))
            if not self._render_timer.isActive():
                self._render_timer.start()
            return True
        return False

    def _clear_selection(self) -> None:
        if any(sel is not None for sel in self._row_selection):
            g.lib.ghostty_terminal_set(self._term, g.TERMINAL_OPT_SELECTION, None)
            if not self._render_timer.isActive():
                self._render_timer.start()

    def _gesture_geometry(self) -> g.SelectionGestureGeometry:
        return g.SelectionGestureGeometry(
            columns=self._cols,
            cell_width=self._cell_w,
            padding_left=0,
            screen_height=self._rows_count * self._cell_h,
        )

    def selection_text(self) -> str:
        """Plain text of the active selection ('' if none)."""
        opts = g.SelectionFormatOptions(
            size=ctypes.sizeof(g.SelectionFormatOptions),
            emit=g.FORMATTER_FORMAT_PLAIN,
            unwrap=True,
            trim=True,
            selection=None,
        )
        ptr = ctypes.POINTER(ctypes.c_uint8)()
        length = ctypes.c_size_t()
        result = g.lib.ghostty_terminal_selection_format_alloc(
            self._term, None, opts, ctypes.byref(ptr), ctypes.byref(length)
        )
        if result != g.SUCCESS or not length.value:
            return ""
        text = ctypes.string_at(ptr, length.value).decode("utf-8", "replace")
        g.lib.ghostty_free(None, ptr, length.value)
        return text

    def copy_selection(self, primary: bool = False) -> bool:
        from PySide6.QtGui import QClipboard

        text = self.selection_text()
        if not text:
            return False
        mode = QClipboard.Mode.Selection if primary else QClipboard.Mode.Clipboard
        clipboard = QGuiApplication.clipboard()
        if primary and not clipboard.supportsSelection():
            return True
        clipboard.setText(text, mode)
        return True

    def _gesture_press(self, ev) -> None:
        import time

        ref = self._grid_ref_at(ev.position())
        if ref is None:
            return
        self._selecting = True
        self._last_mouse_pos = ev.position()
        pos = g.SurfacePosition(x=ev.position().x(), y=ev.position().y())
        now = ctypes.c_uint64(time.monotonic_ns())
        interval = ctypes.c_uint64(
            QGuiApplication.styleHints().mouseDoubleClickInterval() * 1_000_000
        )
        distance = ctypes.c_double(6.0)
        produced = self._fire_gesture(
            g.GESTURE_EVENT_PRESS,
            [
                (g.GESTURE_OPT_REF, ref),
                (g.GESTURE_OPT_POSITION, pos),
                (g.GESTURE_OPT_TIME_NS, now),
                (g.GESTURE_OPT_REPEAT_INTERVAL_NS, interval),
                (g.GESTURE_OPT_REPEAT_DISTANCE, distance),
            ],
        )
        if not produced:
            # Single-click press: no selection yet, clear any existing one.
            self._clear_selection()

    def _autoscroll_tick(self) -> None:
        if not self._selecting or self._last_mouse_pos is None:
            self._autoscroll_timer.stop()
            return
        pos = self._last_mouse_pos
        x = min(max(int(pos.x()) // self._cell_w, 0), self._cols - 1)
        y = min(max(int(pos.y()) // self._cell_h, 0), self._rows_count - 1)
        viewport = g.PointCoordinate(x=x, y=y)
        surface = g.SurfacePosition(x=pos.x(), y=pos.y())
        self._fire_gesture(
            g.GESTURE_EVENT_AUTOSCROLL_TICK,
            [
                (g.GESTURE_OPT_VIEWPORT, viewport),
                (g.GESTURE_OPT_POSITION, surface),
                (g.GESTURE_OPT_GEOMETRY, self._gesture_geometry()),
            ],
        )
        if not self._render_timer.isActive():
            self._render_timer.start()

    def _update_autoscroll(self) -> None:
        state = ctypes.c_int(g.GESTURE_AUTOSCROLL_NONE)
        g.lib.ghostty_selection_gesture_get(
            self._gesture, self._term, g.GESTURE_DATA_AUTOSCROLL, ctypes.byref(state)
        )
        if state.value != g.GESTURE_AUTOSCROLL_NONE:
            if not self._autoscroll_timer.isActive():
                self._autoscroll_timer.start()
        else:
            self._autoscroll_timer.stop()

    # ---- Mouse events ----------------------------------------------------

    def mousePressEvent(self, ev) -> None:
        self.setFocus()
        if ev.button() == Qt.MouseButton.LeftButton:
            self._gesture_press(ev)
        elif ev.button() == Qt.MouseButton.MiddleButton:
            clipboard = QGuiApplication.clipboard()
            from PySide6.QtGui import QClipboard

            self._paste(clipboard.text(QClipboard.Mode.Selection))
        ev.accept()

    def mouseDoubleClickEvent(self, ev) -> None:
        # The gesture engine counts clicks itself (word/line selection);
        # feed Qt's double-click through the same press path.
        if ev.button() == Qt.MouseButton.LeftButton:
            self._gesture_press(ev)
        ev.accept()

    def mouseMoveEvent(self, ev) -> None:
        if not (self._selecting and ev.buttons() & Qt.MouseButton.LeftButton):
            return
        self._last_mouse_pos = ev.position()
        ref = self._grid_ref_at(ev.position())
        if ref is None:
            return
        pos = g.SurfacePosition(x=ev.position().x(), y=ev.position().y())
        self._fire_gesture(
            g.GESTURE_EVENT_DRAG,
            [
                (g.GESTURE_OPT_REF, ref),
                (g.GESTURE_OPT_POSITION, pos),
                (g.GESTURE_OPT_GEOMETRY, self._gesture_geometry()),
            ],
        )
        self._update_autoscroll()
        ev.accept()

    def mouseReleaseEvent(self, ev) -> None:
        if ev.button() == Qt.MouseButton.LeftButton and self._selecting:
            self._selecting = False
            self._autoscroll_timer.stop()
            ref = self._grid_ref_at(ev.position())
            opts = [(g.GESTURE_OPT_REF, ref)] if ref is not None else []
            self._fire_gesture(g.GESTURE_EVENT_RELEASE, opts)
            # X11 convention: a completed selection lands in the primary
            # selection buffer for middle-click paste.
            self.copy_selection(primary=True)
        ev.accept()

    def wheelEvent(self, ev) -> None:
        lines = ev.angleDelta().y() // 40  # 3 lines per notch
        if lines:
            sv = g.ScrollViewport(tag=g.SCROLL_VIEWPORT_DELTA)
            sv.value.delta = -lines
            g.lib.ghostty_terminal_scroll_viewport(self._term, sv)
            if not self._render_timer.isActive():
                self._render_timer.start()
        ev.accept()

    def _scroll(self, tag: int) -> None:
        sv = g.ScrollViewport(tag=tag)
        g.lib.ghostty_terminal_scroll_viewport(self._term, sv)

    def focusInEvent(self, ev) -> None:
        super().focusInEvent(ev)
        self.update()

    def focusOutEvent(self, ev) -> None:
        super().focusOutEvent(ev)
        self.update()
