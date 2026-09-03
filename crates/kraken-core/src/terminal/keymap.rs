//! Qt key events into the bytes a terminal expects.
//!
//! Kraken handed this to libghostty's own encoder. Kraken has no libghostty, so
//! the encoding is written out here — which is not the loss it sounds like: it
//! is a small, entirely table-driven translation, and having it in Rust means
//! every sequence below is asserted in a test rather than trusted.
//!
//! The dialect is xterm's, because that is what `TERM=xterm-256color` promises
//! and what readline, vim and every TUI in between actually read. Two terminal
//! modes change what a key produces — application cursor keys (DECCKM) and
//! bracketed paste (2004) — so both are parameters here rather than assumptions.

/// Qt's `Qt::Key` values, which are stable numbers in Qt's public ABI. QML
/// hands them across unchanged, so they are named here rather than duplicated
/// as magic numbers at the boundary.
pub mod key {
    pub const ESCAPE: u32 = 0x0100_0000;
    pub const TAB: u32 = 0x0100_0001;
    pub const BACKTAB: u32 = 0x0100_0002;
    pub const BACKSPACE: u32 = 0x0100_0003;
    pub const RETURN: u32 = 0x0100_0004;
    pub const ENTER: u32 = 0x0100_0005;
    pub const INSERT: u32 = 0x0100_0006;
    pub const DELETE: u32 = 0x0100_0007;
    pub const PAUSE: u32 = 0x0100_0008;
    pub const PRINT: u32 = 0x0100_0009;
    pub const HOME: u32 = 0x0100_0010;
    pub const END: u32 = 0x0100_0011;
    pub const LEFT: u32 = 0x0100_0012;
    pub const UP: u32 = 0x0100_0013;
    pub const RIGHT: u32 = 0x0100_0014;
    pub const DOWN: u32 = 0x0100_0015;
    pub const PAGE_UP: u32 = 0x0100_0016;
    pub const PAGE_DOWN: u32 = 0x0100_0017;
    pub const F1: u32 = 0x0100_0030;
    pub const F35: u32 = F1 + 34;
}

/// Qt's `Qt::KeyboardModifier` flags, likewise part of its public ABI.
pub mod mods {
    pub const NONE: u32 = 0x0000_0000;
    pub const SHIFT: u32 = 0x0200_0000;
    pub const CONTROL: u32 = 0x0400_0000;
    pub const ALT: u32 = 0x0800_0000;
    pub const META: u32 = 0x1000_0000;
    pub const KEYPAD: u32 = 0x2000_0000;
}

/// One key press, as QML reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyEvent {
    /// A `Qt::Key` value.
    pub key: u32,
    /// A mask of [`mods`] flags.
    pub modifiers: u32,
    /// The text Qt resolved for the key — already through the keyboard layout,
    /// dead keys and the input method, which is why it is preferred over the
    /// key code for anything printable.
    pub text: String,
}

impl KeyEvent {
    pub fn new(key: u32, modifiers: u32, text: &str) -> Self {
        Self {
            key,
            modifiers,
            text: text.to_string(),
        }
    }

    fn has(&self, flag: u32) -> bool {
        self.modifiers & flag != 0
    }
}

/// The terminal state that changes what a key encodes to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modes {
    /// DECCKM: cursor keys send `SS3` instead of `CSI`.
    pub app_cursor: bool,
    /// DECPAM: the numeric keypad sends its application sequences.
    pub app_keypad: bool,
}

/// xterm's modifier parameter: 1 + a bitmask of shift, alt and control, sent as
/// the second parameter of a CSI sequence. 1 means "no modifiers", in which case
/// the parameter is omitted entirely.
fn modifier_param(event: &KeyEvent) -> u32 {
    let mut value = 0;
    if event.has(mods::SHIFT) {
        value |= 1;
    }
    if event.has(mods::ALT) {
        value |= 2;
    }
    if event.has(mods::CONTROL) {
        value |= 4;
    }
    if event.has(mods::META) {
        value |= 8;
    }
    value + 1
}

/// A cursor-style key: `CSI <letter>`, or `SS3 <letter>` in application mode —
/// but *never* SS3 once a modifier is held, because the SS3 form has nowhere to
/// put the parameter.
fn cursor_key(letter: u8, event: &KeyEvent, modes: Modes) -> Vec<u8> {
    let param = modifier_param(event);
    if param > 1 {
        return format!("\x1b[1;{param}{}", letter as char).into_bytes();
    }
    if modes.app_cursor {
        return vec![0x1b, b'O', letter];
    }
    vec![0x1b, b'[', letter]
}

/// A key encoded as `CSI <number> ~`, with the modifier parameter when one is
/// held.
fn tilde_key(number: u32, event: &KeyEvent) -> Vec<u8> {
    let param = modifier_param(event);
    if param > 1 {
        format!("\x1b[{number};{param}~").into_bytes()
    } else {
        format!("\x1b[{number}~").into_bytes()
    }
}

/// F1-F4 are `SS3 P`-`SS3 S` unmodified and `CSI 1;<mods> P`-`S` otherwise;
/// F5 upward are tilde keys with a number that skips a few values, as xterm's
/// have since the sequences were laid out around a keyboard that no longer
/// exists.
fn function_key(n: u32, event: &KeyEvent) -> Vec<u8> {
    if (1..=4).contains(&n) {
        let letter = b'P' + (n - 1) as u8;
        let param = modifier_param(event);
        return if param > 1 {
            format!("\x1b[1;{param}{}", letter as char).into_bytes()
        } else {
            vec![0x1b, b'O', letter]
        };
    }
    let number = match n {
        5..=10 => n + 10,  // F5..F10  -> 15..20, skipping 16
        11..=14 => n + 12, // F11..F14 -> 23..26
        15..=16 => n + 13, // F15..F16 -> 28..29
        17..=20 => n + 14, // F17..F20 -> 31..34
        _ => return Vec::new(),
    };
    let number = if (6..=10).contains(&n) { number + 1 } else { number };
    tilde_key(number, event)
}

/// The control character a `Ctrl`ed key produces, or `None` for a combination
/// that has none.
///
/// The table is xterm's: the letters map to 1-26, and the handful of symbols
/// that also carry control codes are here because `Ctrl+[` is Escape and
/// `Ctrl+C` is not the only thing a shell reads.
fn control_byte(ch: char) -> Option<u8> {
    let byte = match ch.to_ascii_lowercase() {
        'a'..='z' => ch.to_ascii_lowercase() as u8 - b'a' + 1,
        '@' | ' ' => 0,
        '[' => 0x1b,
        '\\' => 0x1c,
        ']' => 0x1d,
        '^' => 0x1e,
        '_' | '/' => 0x1f,
        '?' => 0x7f,
        // Ctrl+2..Ctrl+8 are the control codes their symbols share.
        '2' => 0,
        '3' => 0x1b,
        '4' => 0x1c,
        '5' => 0x1d,
        '6' => 0x1e,
        '7' => 0x1f,
        '8' => 0x7f,
        _ => return None,
    };
    Some(byte)
}

/// The bytes one key press sends, or empty for a key that sends nothing (a bare
/// modifier, an unmapped key).
pub fn encode(event: &KeyEvent, modes: Modes) -> Vec<u8> {
    // Alt is an ESC prefix on everything it does not otherwise change. It is
    // applied last, around whatever the key itself produced, so Alt+Left is
    // parameterised rather than prefixed.
    let alt_prefix = |bytes: Vec<u8>| -> Vec<u8> {
        if event.has(mods::ALT) && !bytes.is_empty() {
            let mut out = vec![0x1b];
            out.extend(bytes);
            out
        } else {
            bytes
        }
    };

    match event.key {
        key::UP => return cursor_key(b'A', event, modes),
        key::DOWN => return cursor_key(b'B', event, modes),
        key::RIGHT => return cursor_key(b'C', event, modes),
        key::LEFT => return cursor_key(b'D', event, modes),
        // Home and End have a CSI H / CSI F form as well as a tilde one; xterm
        // sends the letter form, and so does every terminal a shell was tested
        // against.
        key::HOME => return cursor_key(b'H', event, modes),
        key::END => return cursor_key(b'F', event, modes),
        key::INSERT => return tilde_key(2, event),
        key::DELETE => return tilde_key(3, event),
        key::PAGE_UP => return tilde_key(5, event),
        key::PAGE_DOWN => return tilde_key(6, event),
        key::ESCAPE => return alt_prefix(vec![0x1b]),
        // Shift+Tab is a back-tab, which is its own sequence rather than a
        // modified Tab.
        key::BACKTAB => return b"\x1b[Z".to_vec(),
        key::TAB => {
            if event.has(mods::SHIFT) {
                return b"\x1b[Z".to_vec();
            }
            return alt_prefix(vec![b'\t']);
        }
        // DEL rather than BS: that is what `stty erase ^?` expects, and what
        // every Linux terminal has sent since the choice stopped being a
        // choice. Ctrl+Backspace is the other one, for a word-wise erase.
        key::BACKSPACE => {
            let byte = if event.has(mods::CONTROL) { 0x08 } else { 0x7f };
            return alt_prefix(vec![byte]);
        }
        key::RETURN | key::ENTER => return alt_prefix(vec![b'\r']),
        key::F1..=key::F35 => return function_key(event.key - key::F1 + 1, event),
        _ => {}
    }

    // Control combinations are resolved from the *unmodified* character, since
    // Qt reports `Ctrl+C` with a text of "\u{3}" already on some platforms and
    // with "c" on others.
    if event.has(mods::CONTROL) {
        let ch = char::from_u32(event.key)
            .filter(char::is_ascii)
            .or_else(|| event.text.chars().next())
            .unwrap_or('\0');
        if let Some(byte) = control_byte(ch) {
            return alt_prefix(vec![byte]);
        }
        // A control combination with no code sends nothing, rather than sending
        // the bare letter and typing it into the shell.
        return Vec::new();
    }

    // Anything else is text, if Qt resolved any: that has already been through
    // the keyboard layout, the dead keys and the input method, none of which a
    // key code knows about.
    let printable = !event.text.is_empty()
        && event
            .text
            .chars()
            .all(|ch| ch >= ' ' && ch != '\u{7f}');
    if printable {
        return alt_prefix(event.text.as_bytes().to_vec());
    }
    Vec::new()
}

/// Text pasted into the terminal.
///
/// With bracketed paste on, the text is wrapped in the markers that tell the
/// program it was pasted rather than typed — which is what stops an editor from
/// auto-indenting every line of it into a staircase. Carriage returns are
/// normalised either way: a pasted `\r\n` would otherwise submit twice.
pub fn encode_paste(text: &str, bracketed: bool) -> Vec<u8> {
    let normalised = text.replace("\r\n", "\r").replace('\n', "\r");
    if !bracketed {
        return normalised.into_bytes();
    }
    let mut out = b"\x1b[200~".to_vec();
    // The end marker inside the text would end the paste early, and everything
    // after it would be read as typed input.
    out.extend(normalised.replace("\x1b[201~", "").as_bytes());
    out.extend(b"\x1b[201~");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(key: u32, modifiers: u32, text: &str) -> Vec<u8> {
        encode(&KeyEvent::new(key, modifiers, text), Modes::default())
    }

    fn app(key: u32, modifiers: u32) -> Vec<u8> {
        encode(
            &KeyEvent::new(key, modifiers, ""),
            Modes {
                app_cursor: true,
                app_keypad: false,
            },
        )
    }

    fn text_of(bytes: Vec<u8>) -> String {
        String::from_utf8(bytes).expect("valid utf-8")
    }

    #[test]
    fn a_plain_letter_is_the_text_qt_resolved() {
        assert_eq!(bytes('A' as u32, mods::NONE, "a"), b"a");
        assert_eq!(bytes('A' as u32, mods::SHIFT, "A"), b"A");
        // A layout that produces something else for the same key code wins:
        // that is the whole reason the text is preferred over the code.
        assert_eq!(bytes('Q' as u32, mods::NONE, "ä"), "ä".as_bytes());
    }

    #[test]
    fn control_letters_are_the_codes_one_to_twenty_six() {
        assert_eq!(bytes('C' as u32, mods::CONTROL, "c"), vec![3]);
        assert_eq!(bytes('A' as u32, mods::CONTROL, "a"), vec![1]);
        assert_eq!(bytes('Z' as u32, mods::CONTROL, "z"), vec![26]);
        // Qt sometimes reports the control character as the text already.
        assert_eq!(bytes('D' as u32, mods::CONTROL, "\u{4}"), vec![4]);
    }

    #[test]
    fn the_control_symbols_carry_their_own_codes() {
        assert_eq!(bytes('[' as u32, mods::CONTROL, "["), vec![0x1b]);
        assert_eq!(bytes('\\' as u32, mods::CONTROL, "\\"), vec![0x1c]);
        assert_eq!(bytes(' ' as u32, mods::CONTROL, " "), vec![0]);
        assert_eq!(bytes('/' as u32, mods::CONTROL, "/"), vec![0x1f]);
    }

    #[test]
    fn a_control_combination_with_no_code_sends_nothing() {
        // Rather than sending the bare letter, which would type it.
        // Qt::Key_Super_L, which is past the function-key block.
        assert!(bytes(0x0100_0053, mods::CONTROL, "").is_empty());
        assert!(bytes('§' as u32, mods::CONTROL, "§").is_empty());
    }

    #[test]
    fn the_arrows_are_csi_letters_and_ss3_in_application_mode() {
        assert_eq!(text_of(bytes(key::UP, mods::NONE, "")), "\x1b[A");
        assert_eq!(text_of(bytes(key::DOWN, mods::NONE, "")), "\x1b[B");
        assert_eq!(text_of(bytes(key::RIGHT, mods::NONE, "")), "\x1b[C");
        assert_eq!(text_of(bytes(key::LEFT, mods::NONE, "")), "\x1b[D");
        assert_eq!(text_of(app(key::UP, mods::NONE)), "\x1bOA");
        assert_eq!(text_of(app(key::LEFT, mods::NONE)), "\x1bOD");
    }

    #[test]
    fn a_modified_arrow_is_parameterised_even_in_application_mode() {
        // The SS3 form has nowhere to put the parameter, so a modifier drops
        // back to CSI whatever the mode says.
        assert_eq!(text_of(bytes(key::LEFT, mods::CONTROL, "")), "\x1b[1;5D");
        assert_eq!(text_of(app(key::LEFT, mods::CONTROL)), "\x1b[1;5D");
        assert_eq!(text_of(bytes(key::RIGHT, mods::SHIFT, "")), "\x1b[1;2C");
        assert_eq!(text_of(bytes(key::UP, mods::ALT, "")), "\x1b[1;3A");
        // Shift+Ctrl is 1 + 1 + 4.
        assert_eq!(
            text_of(bytes(key::UP, mods::SHIFT | mods::CONTROL, "")),
            "\x1b[1;6A"
        );
    }

    #[test]
    fn home_and_end_send_the_letter_form_xterm_does() {
        assert_eq!(text_of(bytes(key::HOME, mods::NONE, "")), "\x1b[H");
        assert_eq!(text_of(bytes(key::END, mods::NONE, "")), "\x1b[F");
        assert_eq!(text_of(bytes(key::HOME, mods::CONTROL, "")), "\x1b[1;5H");
    }

    #[test]
    fn the_navigation_block_sends_tilde_sequences() {
        assert_eq!(text_of(bytes(key::INSERT, mods::NONE, "")), "\x1b[2~");
        assert_eq!(text_of(bytes(key::DELETE, mods::NONE, "")), "\x1b[3~");
        assert_eq!(text_of(bytes(key::PAGE_UP, mods::NONE, "")), "\x1b[5~");
        assert_eq!(text_of(bytes(key::PAGE_DOWN, mods::NONE, "")), "\x1b[6~");
        assert_eq!(text_of(bytes(key::DELETE, mods::SHIFT, "")), "\x1b[3;2~");
    }

    #[test]
    fn the_function_keys_follow_xterms_two_shapes() {
        // F1-F4 are SS3 letters; F5 up are tilde keys, with the gaps xterm
        // left in the numbering.
        assert_eq!(text_of(bytes(key::F1, mods::NONE, "")), "\x1bOP");
        assert_eq!(text_of(bytes(key::F1 + 3, mods::NONE, "")), "\x1bOS");
        assert_eq!(text_of(bytes(key::F1 + 4, mods::NONE, "")), "\x1b[15~");
        assert_eq!(text_of(bytes(key::F1 + 5, mods::NONE, "")), "\x1b[17~");
        assert_eq!(text_of(bytes(key::F1 + 9, mods::NONE, "")), "\x1b[21~");
        assert_eq!(text_of(bytes(key::F1 + 10, mods::NONE, "")), "\x1b[23~");
        // A modifier moves F1-F4 to the parameterised CSI form.
        assert_eq!(text_of(bytes(key::F1, mods::SHIFT, "")), "\x1b[1;2P");
    }

    #[test]
    fn enter_backspace_and_tab_send_what_a_shell_reads() {
        assert_eq!(bytes(key::RETURN, mods::NONE, "\r"), b"\r");
        assert_eq!(bytes(key::ENTER, mods::NONE, "\r"), b"\r");
        assert_eq!(bytes(key::TAB, mods::NONE, "\t"), b"\t");
        // DEL, not BS: `stty erase ^?` is what every Linux terminal promises.
        assert_eq!(bytes(key::BACKSPACE, mods::NONE, ""), vec![0x7f]);
        assert_eq!(bytes(key::BACKSPACE, mods::CONTROL, ""), vec![0x08]);
    }

    #[test]
    fn shift_tab_is_a_back_tab_rather_than_a_modified_tab() {
        assert_eq!(text_of(bytes(key::TAB, mods::SHIFT, "\t")), "\x1b[Z");
        assert_eq!(text_of(bytes(key::BACKTAB, mods::SHIFT, "")), "\x1b[Z");
    }

    #[test]
    fn alt_prefixes_with_escape_what_it_does_not_otherwise_change() {
        assert_eq!(text_of(bytes('B' as u32, mods::ALT, "b")), "\x1bb");
        assert_eq!(text_of(bytes(key::RETURN, mods::ALT, "\r")), "\x1b\r");
        assert_eq!(bytes(key::BACKSPACE, mods::ALT, ""), vec![0x1b, 0x7f]);
        // Alt+Ctrl is ESC then the control code.
        assert_eq!(bytes('C' as u32, mods::ALT | mods::CONTROL, "c"), vec![0x1b, 3]);
        // But an arrow is parameterised, not prefixed.
        assert_eq!(text_of(bytes(key::LEFT, mods::ALT, "")), "\x1b[1;3D");
    }

    #[test]
    fn a_key_that_resolved_to_no_text_sends_nothing() {
        // A bare modifier press, and a key Qt gave no text for.
        assert!(bytes(0x0100_0020, mods::NONE, "").is_empty());
        assert!(bytes('A' as u32, mods::NONE, "").is_empty());
    }

    #[test]
    fn a_plain_paste_is_the_text_with_its_newlines_normalised() {
        assert_eq!(encode_paste("a\nb", false), b"a\rb");
        assert_eq!(encode_paste("a\r\nb", false), b"a\rb");
        assert_eq!(encode_paste("plain", false), b"plain");
    }

    #[test]
    fn bracketed_paste_wraps_the_text_in_its_markers() {
        assert_eq!(
            text_of(encode_paste("a\nb", true)),
            "\x1b[200~a\rb\x1b[201~"
        );
    }

    #[test]
    fn a_paste_cannot_smuggle_its_own_end_marker() {
        // Otherwise everything after it would be read as typed input, which is
        // how a pasted line becomes a run command.
        let pasted = encode_paste("safe\x1b[201~rm -rf /", true);
        assert_eq!(text_of(pasted), "\x1b[200~saferm -rf /\x1b[201~");
    }
}
