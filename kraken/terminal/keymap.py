"""Static keyboard-mapping tables for the terminal widget.

Two lookup tables translating Qt key events into the names and codepoints
libghostty-vt's key encoder expects. Kept apart from the widget logic
because they are large, static, and change independently of it.
"""

from __future__ import annotations

from PySide6.QtCore import Qt

# Qt key -> GhosttyKey name (W3C physical-ish mapping from logical Qt keys).
QT_KEY_MAP = {
    Qt.Key.Key_Escape: "ESCAPE",
    Qt.Key.Key_Tab: "TAB",
    Qt.Key.Key_Backtab: "TAB",
    Qt.Key.Key_Backspace: "BACKSPACE",
    Qt.Key.Key_Return: "ENTER",
    Qt.Key.Key_Enter: "NUMPAD_ENTER",
    Qt.Key.Key_Insert: "INSERT",
    Qt.Key.Key_Delete: "DELETE",
    Qt.Key.Key_Home: "HOME",
    Qt.Key.Key_End: "END",
    Qt.Key.Key_PageUp: "PAGE_UP",
    Qt.Key.Key_PageDown: "PAGE_DOWN",
    Qt.Key.Key_Up: "ARROW_UP",
    Qt.Key.Key_Down: "ARROW_DOWN",
    Qt.Key.Key_Left: "ARROW_LEFT",
    Qt.Key.Key_Right: "ARROW_RIGHT",
    Qt.Key.Key_Space: "SPACE",
    Qt.Key.Key_Minus: "MINUS",
    Qt.Key.Key_Equal: "EQUAL",
    Qt.Key.Key_BracketLeft: "BRACKET_LEFT",
    Qt.Key.Key_BracketRight: "BRACKET_RIGHT",
    Qt.Key.Key_Backslash: "BACKSLASH",
    Qt.Key.Key_Semicolon: "SEMICOLON",
    Qt.Key.Key_Apostrophe: "QUOTE",
    Qt.Key.Key_QuoteLeft: "BACKQUOTE",
    Qt.Key.Key_Comma: "COMMA",
    Qt.Key.Key_Period: "PERIOD",
    Qt.Key.Key_Slash: "SLASH",
    Qt.Key.Key_CapsLock: "CAPS_LOCK",
    Qt.Key.Key_NumLock: "NUM_LOCK",
    Qt.Key.Key_ScrollLock: "SCROLL_LOCK",
    Qt.Key.Key_Print: "PRINT_SCREEN",
    Qt.Key.Key_Pause: "PAUSE",
    Qt.Key.Key_Menu: "CONTEXT_MENU",
    Qt.Key.Key_Shift: "SHIFT_LEFT",
    Qt.Key.Key_Control: "CONTROL_LEFT",
    Qt.Key.Key_Alt: "ALT_LEFT",
    Qt.Key.Key_AltGr: "ALT_RIGHT",
    Qt.Key.Key_Meta: "META_LEFT",
}
for _i in range(10):
    QT_KEY_MAP[Qt.Key(Qt.Key.Key_0 + _i)] = f"DIGIT_{_i}"
for _i in range(26):
    QT_KEY_MAP[Qt.Key(Qt.Key.Key_A + _i)] = chr(ord("A") + _i)
for _i in range(25):
    QT_KEY_MAP[Qt.Key(Qt.Key.Key_F1 + _i)] = f"F{_i + 1}"

# GhosttyKey name -> unshifted codepoint, for keys that produce text.
UNSHIFTED = {f"DIGIT_{i}": ord(str(i)) for i in range(10)}
UNSHIFTED.update({chr(c): ord(chr(c).lower()) for c in range(ord("A"), ord("Z") + 1)})
UNSHIFTED.update(
    {
        "SPACE": ord(" "), "MINUS": ord("-"), "EQUAL": ord("="),
        "BRACKET_LEFT": ord("["), "BRACKET_RIGHT": ord("]"),
        "BACKSLASH": ord("\\"), "SEMICOLON": ord(";"), "QUOTE": ord("'"),
        "BACKQUOTE": ord("`"), "COMMA": ord(","), "PERIOD": ord("."),
        "SLASH": ord("/"),
    }
)
