"""ctypes bindings for the subset of libghostty-vt used by the terminal widget.

libghostty-vt is Ghostty's embeddable terminal-core C library (terminal state,
VT stream parsing, render-state extraction, key encoding). It is built from
the vendored Ghostty source: vendor/ghostty (zig build -Demit-lib-vt).
"""

from __future__ import annotations

import ctypes
import json
import os
from ctypes import (
    CFUNCTYPE,
    POINTER,
    Structure,
    Union,
    c_bool,
    c_char_p,
    c_int,
    c_size_t,
    c_ssize_t,
    c_uint8,
    c_uint16,
    c_uint32,
    c_uint64,
    c_void_p,
)

_LIB_PATH = os.path.normpath(
    os.path.join(
        os.path.dirname(__file__),
        "..",
        "vendor",
        "ghostty",
        "zig-out",
        "lib",
        "libghostty-vt.so",
    )
)

lib = ctypes.CDLL(_LIB_PATH)

# GhosttyResult
SUCCESS = 0
OUT_OF_MEMORY = -1
INVALID_VALUE = -2
OUT_OF_SPACE = -3
NO_VALUE = -4


class ColorRgb(Structure):
    _fields_ = [("r", c_uint8), ("g", c_uint8), ("b", c_uint8)]


class StyleColorValue(Union):
    _fields_ = [("palette", c_uint8), ("rgb", ColorRgb), ("_padding", c_uint64)]


STYLE_COLOR_NONE = 0
STYLE_COLOR_PALETTE = 1
STYLE_COLOR_RGB = 2


class StyleColor(Structure):
    _fields_ = [("tag", c_int), ("value", StyleColorValue)]


class Style(Structure):
    _fields_ = [
        ("size", c_size_t),
        ("fg_color", StyleColor),
        ("bg_color", StyleColor),
        ("underline_color", StyleColor),
        ("bold", c_bool),
        ("italic", c_bool),
        ("faint", c_bool),
        ("blink", c_bool),
        ("inverse", c_bool),
        ("invisible", c_bool),
        ("strikethrough", c_bool),
        ("overline", c_bool),
        ("underline", c_int),
    ]


class TerminalOptions(Structure):
    _fields_ = [("cols", c_uint16), ("rows", c_uint16), ("max_scrollback", c_size_t)]


class RenderStateColors(Structure):
    _fields_ = [
        ("size", c_size_t),
        ("background", ColorRgb),
        ("foreground", ColorRgb),
        ("cursor", ColorRgb),
        ("cursor_has_value", c_bool),
        ("palette", ColorRgb * 256),
    ]


class RowSelection(Structure):
    _fields_ = [("size", c_size_t), ("start_x", c_uint16), ("end_x", c_uint16)]


class Buffer(Structure):
    _fields_ = [("ptr", c_void_p), ("cap", c_size_t), ("len", c_size_t)]


class PointCoordinate(Structure):
    _fields_ = [("x", c_uint16), ("y", c_uint32)]


class PointValue(Union):
    _fields_ = [("coordinate", PointCoordinate), ("_padding", c_uint64 * 2)]


POINT_TAG_ACTIVE = 0
POINT_TAG_VIEWPORT = 1
POINT_TAG_SCREEN = 2
POINT_TAG_HISTORY = 3


class Point(Structure):
    _fields_ = [("tag", c_int), ("value", PointValue)]


class GridRef(Structure):
    _fields_ = [
        ("size", c_size_t),
        ("node", c_void_p),
        ("x", c_uint16),
        ("y", c_uint16),
    ]


class Selection(Structure):
    _fields_ = [
        ("size", c_size_t),
        ("start", GridRef),
        ("end", GridRef),
        ("rectangle", c_bool),
    ]


class SurfacePosition(Structure):
    _fields_ = [("x", ctypes.c_double), ("y", ctypes.c_double)]


class SelectionGestureGeometry(Structure):
    _fields_ = [
        ("columns", c_uint32),
        ("cell_width", c_uint32),
        ("padding_left", c_uint32),
        ("screen_height", c_uint32),
    ]


FORMATTER_FORMAT_PLAIN = 0


class SelectionFormatOptions(Structure):
    _fields_ = [
        ("size", c_size_t),
        ("emit", c_int),
        ("unwrap", c_bool),
        ("trim", c_bool),
        ("selection", c_void_p),
    ]


# Selection gesture event types
GESTURE_EVENT_PRESS = 0
GESTURE_EVENT_RELEASE = 1
GESTURE_EVENT_DRAG = 2
GESTURE_EVENT_AUTOSCROLL_TICK = 3

# Selection gesture event options
GESTURE_OPT_REF = 0
GESTURE_OPT_POSITION = 1
GESTURE_OPT_REPEAT_DISTANCE = 2
GESTURE_OPT_TIME_NS = 3
GESTURE_OPT_REPEAT_INTERVAL_NS = 4
GESTURE_OPT_GEOMETRY = 8
GESTURE_OPT_VIEWPORT = 9

# Selection gesture data
GESTURE_DATA_CLICK_COUNT = 0
GESTURE_DATA_DRAGGED = 1
GESTURE_DATA_AUTOSCROLL = 2

GESTURE_AUTOSCROLL_NONE = 0
GESTURE_AUTOSCROLL_UP = 1
GESTURE_AUTOSCROLL_DOWN = 2


class ScrollViewportValue(Union):
    _fields_ = [("delta", c_ssize_t), ("row", c_size_t), ("_padding", c_uint64 * 2)]


SCROLL_VIEWPORT_TOP = 0
SCROLL_VIEWPORT_BOTTOM = 1
SCROLL_VIEWPORT_DELTA = 2
SCROLL_VIEWPORT_ROW = 3


class ScrollViewport(Structure):
    _fields_ = [("tag", c_int), ("value", ScrollViewportValue)]


# Terminal options (ghostty_terminal_set)
TERMINAL_OPT_SELECTION = 21
TERMINAL_OPT_USERDATA = 0
TERMINAL_OPT_WRITE_PTY = 1
TERMINAL_OPT_BELL = 2
TERMINAL_OPT_TITLE_CHANGED = 5
TERMINAL_OPT_COLOR_FOREGROUND = 11
TERMINAL_OPT_COLOR_BACKGROUND = 12
TERMINAL_OPT_COLOR_CURSOR = 13
TERMINAL_OPT_COLOR_PALETTE = 14

# Terminal data (ghostty_terminal_get)
TERMINAL_DATA_TITLE = 12

# Render state data (ghostty_render_state_get)
RENDER_STATE_DATA_COLS = 1
RENDER_STATE_DATA_ROWS = 2
RENDER_STATE_DATA_DIRTY = 3
RENDER_STATE_DATA_ROW_ITERATOR = 4
RENDER_STATE_DATA_CURSOR_VISUAL_STYLE = 10
RENDER_STATE_DATA_CURSOR_VISIBLE = 11
RENDER_STATE_DATA_CURSOR_VIEWPORT_HAS_VALUE = 14
RENDER_STATE_DATA_CURSOR_VIEWPORT_X = 15
RENDER_STATE_DATA_CURSOR_VIEWPORT_Y = 16

RENDER_STATE_DIRTY_FALSE = 0
RENDER_STATE_DIRTY_PARTIAL = 1
RENDER_STATE_DIRTY_FULL = 2

RENDER_STATE_OPTION_DIRTY = 0

CURSOR_STYLE_BAR = 0
CURSOR_STYLE_BLOCK = 1
CURSOR_STYLE_UNDERLINE = 2
CURSOR_STYLE_BLOCK_HOLLOW = 3

# Row data (ghostty_render_state_row_get)
ROW_DATA_DIRTY = 1
ROW_DATA_CELLS = 3
ROW_DATA_SELECTION = 4

ROW_OPTION_DIRTY = 0

# Cell data (ghostty_render_state_row_cells_get)
CELLS_DATA_STYLE = 2
CELLS_DATA_GRAPHEMES_LEN = 3
CELLS_DATA_BG_COLOR = 5
CELLS_DATA_FG_COLOR = 6
CELLS_DATA_HAS_STYLING = 8
CELLS_DATA_GRAPHEMES_UTF8 = 9

# Key actions
KEY_ACTION_RELEASE = 0
KEY_ACTION_PRESS = 1
KEY_ACTION_REPEAT = 2

# Modifier bitmask
MODS_SHIFT = 1 << 0
MODS_CTRL = 1 << 1
MODS_ALT = 1 << 2
MODS_SUPER = 1 << 3
MODS_CAPS_LOCK = 1 << 4
MODS_NUM_LOCK = 1 << 5

# GhosttyKey values, in declaration order from include/ghostty/vt/key/event.h.
_KEY_NAMES = [
    "UNIDENTIFIED",
    # Writing system keys
    "BACKQUOTE", "BACKSLASH", "BRACKET_LEFT", "BRACKET_RIGHT", "COMMA",
    "DIGIT_0", "DIGIT_1", "DIGIT_2", "DIGIT_3", "DIGIT_4", "DIGIT_5",
    "DIGIT_6", "DIGIT_7", "DIGIT_8", "DIGIT_9", "EQUAL",
    "INTL_BACKSLASH", "INTL_RO", "INTL_YEN",
    "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M",
    "N", "O", "P", "Q", "R", "S", "T", "U", "V", "W", "X", "Y", "Z",
    "MINUS", "PERIOD", "QUOTE", "SEMICOLON", "SLASH",
    # Functional keys
    "ALT_LEFT", "ALT_RIGHT", "BACKSPACE", "CAPS_LOCK", "CONTEXT_MENU",
    "CONTROL_LEFT", "CONTROL_RIGHT", "ENTER", "META_LEFT", "META_RIGHT",
    "SHIFT_LEFT", "SHIFT_RIGHT", "SPACE", "TAB", "CONVERT", "KANA_MODE",
    "NON_CONVERT",
    # Control pad
    "DELETE", "END", "HELP", "HOME", "INSERT", "PAGE_DOWN", "PAGE_UP",
    # Arrows
    "ARROW_DOWN", "ARROW_LEFT", "ARROW_RIGHT", "ARROW_UP",
    # Numpad
    "NUM_LOCK", "NUMPAD_0", "NUMPAD_1", "NUMPAD_2", "NUMPAD_3", "NUMPAD_4",
    "NUMPAD_5", "NUMPAD_6", "NUMPAD_7", "NUMPAD_8", "NUMPAD_9",
    "NUMPAD_ADD", "NUMPAD_BACKSPACE", "NUMPAD_CLEAR", "NUMPAD_CLEAR_ENTRY",
    "NUMPAD_COMMA", "NUMPAD_DECIMAL", "NUMPAD_DIVIDE", "NUMPAD_ENTER",
    "NUMPAD_EQUAL", "NUMPAD_MEMORY_ADD", "NUMPAD_MEMORY_CLEAR",
    "NUMPAD_MEMORY_RECALL", "NUMPAD_MEMORY_STORE", "NUMPAD_MEMORY_SUBTRACT",
    "NUMPAD_MULTIPLY", "NUMPAD_PAREN_LEFT", "NUMPAD_PAREN_RIGHT",
    "NUMPAD_SUBTRACT", "NUMPAD_SEPARATOR", "NUMPAD_UP", "NUMPAD_DOWN",
    "NUMPAD_RIGHT", "NUMPAD_LEFT", "NUMPAD_BEGIN", "NUMPAD_HOME",
    "NUMPAD_END", "NUMPAD_INSERT", "NUMPAD_DELETE", "NUMPAD_PAGE_UP",
    "NUMPAD_PAGE_DOWN",
    # Function section
    "ESCAPE", "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10",
    "F11", "F12", "F13", "F14", "F15", "F16", "F17", "F18", "F19", "F20",
    "F21", "F22", "F23", "F24", "F25", "FN", "FN_LOCK", "PRINT_SCREEN",
    "SCROLL_LOCK", "PAUSE",
    # Media keys
    "BROWSER_BACK", "BROWSER_FAVORITES", "BROWSER_FORWARD", "BROWSER_HOME",
    "BROWSER_REFRESH", "BROWSER_SEARCH", "BROWSER_STOP", "EJECT",
    "LAUNCH_APP_1", "LAUNCH_APP_2", "LAUNCH_MAIL", "MEDIA_PLAY_PAUSE",
    "MEDIA_SELECT", "MEDIA_STOP", "MEDIA_TRACK_NEXT", "MEDIA_TRACK_PREVIOUS",
    "POWER", "SLEEP", "AUDIO_VOLUME_DOWN", "AUDIO_VOLUME_MUTE",
    "AUDIO_VOLUME_UP", "WAKE_UP",
    # Legacy
    "COPY", "CUT", "PASTE",
]

KEY = {name: value for value, name in enumerate(_KEY_NAMES)}


def mode(value: int, ansi: bool = False) -> int:
    """Pack a mode number into a GhosttyMode (uint16)."""
    return (value & 0x7FFF) | (int(ansi) << 15)


MODE_BRACKETED_PASTE = mode(2004)

WritePtyFn = CFUNCTYPE(None, c_void_p, c_void_p, POINTER(c_uint8), c_size_t)

_protos = [
    # Terminal
    ("ghostty_terminal_new", [c_void_p, POINTER(c_void_p), TerminalOptions], c_int),
    ("ghostty_terminal_free", [c_void_p], None),
    ("ghostty_terminal_resize", [c_void_p, c_uint16, c_uint16, c_uint32, c_uint32], c_int),
    ("ghostty_terminal_set", [c_void_p, c_int, c_void_p], c_int),
    ("ghostty_terminal_get", [c_void_p, c_int, c_void_p], c_int),
    ("ghostty_terminal_vt_write", [c_void_p, POINTER(c_uint8), c_size_t], None),
    ("ghostty_terminal_scroll_viewport", [c_void_p, ScrollViewport], None),
    ("ghostty_terminal_mode_get", [c_void_p, c_uint16, POINTER(c_bool)], c_int),
    # Render state
    ("ghostty_render_state_new", [c_void_p, POINTER(c_void_p)], c_int),
    ("ghostty_render_state_free", [c_void_p], None),
    ("ghostty_render_state_update", [c_void_p, c_void_p], c_int),
    ("ghostty_render_state_get", [c_void_p, c_int, c_void_p], c_int),
    ("ghostty_render_state_set", [c_void_p, c_int, c_void_p], c_int),
    ("ghostty_render_state_colors_get", [c_void_p, POINTER(RenderStateColors)], c_int),
    ("ghostty_render_state_row_iterator_new", [c_void_p, POINTER(c_void_p)], c_int),
    ("ghostty_render_state_row_iterator_free", [c_void_p], None),
    ("ghostty_render_state_row_iterator_next", [c_void_p], c_bool),
    ("ghostty_render_state_row_get", [c_void_p, c_int, c_void_p], c_int),
    ("ghostty_render_state_row_set", [c_void_p, c_int, c_void_p], c_int),
    ("ghostty_render_state_row_cells_new", [c_void_p, POINTER(c_void_p)], c_int),
    ("ghostty_render_state_row_cells_free", [c_void_p], None),
    ("ghostty_render_state_row_cells_next", [c_void_p], c_bool),
    ("ghostty_render_state_row_cells_get", [c_void_p, c_int, c_void_p], c_int),
    # Key encoding
    ("ghostty_key_encoder_new", [c_void_p, POINTER(c_void_p)], c_int),
    ("ghostty_key_encoder_free", [c_void_p], None),
    ("ghostty_key_encoder_setopt_from_terminal", [c_void_p, c_void_p], None),
    ("ghostty_key_event_new", [c_void_p, POINTER(c_void_p)], c_int),
    ("ghostty_key_event_free", [c_void_p], None),
    ("ghostty_key_event_set_action", [c_void_p, c_int], None),
    ("ghostty_key_event_set_key", [c_void_p, c_int], None),
    ("ghostty_key_event_set_mods", [c_void_p, c_uint16], None),
    ("ghostty_key_event_set_consumed_mods", [c_void_p, c_uint16], None),
    ("ghostty_key_event_set_utf8", [c_void_p, c_char_p, c_size_t], None),
    ("ghostty_key_event_set_unshifted_codepoint", [c_void_p, c_uint32], None),
    ("ghostty_key_encoder_encode", [c_void_p, c_void_p, c_char_p, c_size_t, POINTER(c_size_t)], c_int),
    # Paste
    ("ghostty_paste_encode", [c_char_p, c_size_t, c_bool, c_char_p, c_size_t, POINTER(c_size_t)], c_int),
    # Selection
    ("ghostty_terminal_grid_ref", [c_void_p, Point, POINTER(GridRef)], c_int),
    ("ghostty_selection_gesture_new", [c_void_p, POINTER(c_void_p)], c_int),
    ("ghostty_selection_gesture_free", [c_void_p, c_void_p], None),
    ("ghostty_selection_gesture_reset", [c_void_p, c_void_p], None),
    ("ghostty_selection_gesture_get", [c_void_p, c_void_p, c_int, c_void_p], c_int),
    ("ghostty_selection_gesture_event_new", [c_void_p, POINTER(c_void_p), c_int], c_int),
    ("ghostty_selection_gesture_event_free", [c_void_p], None),
    ("ghostty_selection_gesture_event_set", [c_void_p, c_int, c_void_p], c_int),
    ("ghostty_selection_gesture_event", [c_void_p, c_void_p, c_void_p, POINTER(Selection)], c_int),
    (
        "ghostty_terminal_selection_format_alloc",
        [c_void_p, c_void_p, SelectionFormatOptions, POINTER(ctypes.POINTER(c_uint8)), POINTER(c_size_t)],
        c_int,
    ),
    ("ghostty_free", [c_void_p, ctypes.POINTER(c_uint8), c_size_t], None),
    # Introspection
    ("ghostty_type_json", [], c_char_p),
]

for _name, _argtypes, _restype in _protos:
    _fn = getattr(lib, _name)
    _fn.argtypes = _argtypes
    _fn.restype = _restype


def _verify_struct_layouts() -> None:
    """Cross-check our ctypes structs against the library's own layout data."""
    layouts = json.loads(lib.ghostty_type_json().decode())
    expected = {
        "GhosttyStyle": Style,
        "GhosttyTerminalOptions": TerminalOptions,
        "GhosttyRenderStateColors": RenderStateColors,
        "GhosttyRenderStateRowSelection": RowSelection,
        "GhosttyBuffer": Buffer,
        "GhosttyColorRgb": ColorRgb,
        "GhosttyTerminalScrollViewport": ScrollViewport,
        "GhosttyPoint": Point,
        "GhosttyGridRef": GridRef,
        "GhosttySelection": Selection,
        "GhosttySurfacePosition": SurfacePosition,
        "GhosttySelectionGestureGeometry": SelectionGestureGeometry,
        "GhosttyTerminalSelectionFormatOptions": SelectionFormatOptions,
    }
    for name, struct in expected.items():
        info = layouts.get(name)
        if info is None:
            continue
        if info["size"] != ctypes.sizeof(struct):
            raise RuntimeError(
                f"libghostty-vt ABI mismatch for {name}: "
                f"C size {info['size']} != ctypes size {ctypes.sizeof(struct)}"
            )


_verify_struct_layouts()
