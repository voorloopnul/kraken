//! The embedded terminal: a pty, a VT engine over it, and the key encoding
//! between them.
//!
//! [`Terminal`] is what the UI owns — one per tab. It runs a reader thread on
//! the pty and feeds what arrives into the engine, so the interface never
//! blocks on a shell, and exposes the engine's render state for painting.
//!
//! Kraken linked libghostty-vt through ctypes for the engine half. Pupo's is
//! written in Rust ([`vt`]) for one reason above the others: the whole thing can
//! be driven from a test by feeding it a byte string, which is how every escape
//! sequence it claims to understand is actually checked.

pub mod keymap;
pub mod pty;
pub mod vt;

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::Arc;
use std::thread::JoinHandle;

use crate::remote::RemoteTarget;
use crate::theme::{terminal_theme, TerminalTheme};
use crate::typography::terminal as sizes;

pub use keymap::{KeyEvent, Modes};
pub use pty::{Command, Grid, Pty};
pub use vt::{CursorStyle, RenderState, SelectionMode, Vt};

/// What the reader thread hands back.
enum FromPty {
    Output(Vec<u8>),
    /// The child hung up. The engine keeps whatever it last drew — a shell that
    /// exits should leave its last screen visible, not a blank one.
    Closed,
}

/// One terminal tab: a shell on a pty, and the screen it is drawing.
pub struct Terminal {
    vt: Vt,
    pty: Option<Pty>,
    reader: Option<JoinHandle<()>>,
    /// Tells the reader thread to stop; it is blocked in `read`, so shutdown
    /// closes the descriptor under it as well.
    stopping: Arc<AtomicBool>,
    output: Receiver<FromPty>,
    theme_name: String,
    font_size: i32,
    /// Cell metrics, measured by the UI from the font it actually rendered —
    /// only it knows what the glyphs came out as.
    cell: (u16, u16),
    closed: bool,
    title: String,
}

impl Terminal {
    /// Start a shell in `cwd`, or an ssh client to `remote` when one is given.
    pub fn spawn(
        cwd: Option<&str>,
        remote: Option<&RemoteTarget>,
        theme_name: &str,
        font_size: i32,
        grid: Grid,
    ) -> io::Result<Self> {
        let command = match remote {
            Some(target) => Command::remote_shell(target),
            None => Command::local_shell(cwd),
        };
        let pty = Pty::spawn(&command, grid)?;
        crate::debug::proc(
            "terminal.spawn",
            &[
                ("pid", pty.child_pid().to_string()),
                ("program", command.program.clone()),
                ("remote", remote.is_some().to_string()),
            ],
        );
        let theme = terminal_theme(theme_name);
        let vt = Vt::new(grid.cols as usize, grid.rows as usize, theme);

        let (sender, output) = mpsc::channel();
        let stopping = Arc::new(AtomicBool::new(false));
        // The thread reads through a dup of the master rather than the Pty's
        // own descriptor: teardown closes the original to break this thread out
        // of its blocking read, and a thread reading a descriptor the parent
        // has already closed would be reading whatever got that number next.
        let fd = pty.dup_master()?;
        let flag = Arc::clone(&stopping);
        let reader = std::thread::Builder::new()
            .name("pupo-pty-reader".into())
            .spawn(move || read_loop(fd, sender, flag))?;

        Ok(Self {
            vt,
            pty: Some(pty),
            reader: Some(reader),
            stopping,
            output,
            theme_name: theme_name.to_string(),
            font_size: sizes::clamp(font_size),
            cell: (grid.cell_w, grid.cell_h),
            closed: false,
            title: String::new(),
        })
    }

    /// Take everything the shell has written since the last call and draw it.
    /// Returns whether anything changed, so the UI can skip a repaint that
    /// would draw the same frame again.
    pub fn pump(&mut self) -> bool {
        let mut changed = false;
        loop {
            match self.output.try_recv() {
                Ok(FromPty::Output(bytes)) => {
                    self.vt.feed(&bytes);
                    changed = true;
                }
                Ok(FromPty::Closed) | Err(TryRecvError::Disconnected) => {
                    if !self.closed {
                        self.closed = true;
                        changed = true;
                    }
                    break;
                }
                Err(TryRecvError::Empty) => break,
            }
        }
        if changed {
            // Terminal queries (cursor position, device attributes) are
            // answered by writing back into the pty, which the engine cannot do
            // itself.
            let responses = self.vt.screen_mut().take_responses();
            if !responses.is_empty() {
                self.write(&responses);
            }
            if let Some(title) = self.vt.screen_mut().take_title() {
                self.title = title;
            }
        }
        changed
    }

    /// Whether the shell has exited. Its last screen stays on display.
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// The title the shell set with an OSC, or empty if it never set one.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// A bell since the last call, for the UI to mark the tab with.
    pub fn take_bell(&mut self) -> bool {
        self.vt.screen_mut().take_bell()
    }

    /// Drive the engine directly, as though the bytes had come from the pty.
    ///
    /// The child is not involved, which is the point: a test can assert on what
    /// a sequence draws without a shell's line discipline echoing its own
    /// version of the bytes back first.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.vt.feed(bytes);
        if let Some(title) = self.vt.screen_mut().take_title() {
            self.title = title;
        }
    }

    pub fn render(&mut self) -> &RenderState {
        self.vt.screen_mut().render()
    }

    pub fn screen(&self) -> &vt::Screen {
        self.vt.screen()
    }

    pub fn screen_mut(&mut self) -> &mut vt::Screen {
        self.vt.screen_mut()
    }

    /// Send bytes to the shell. A dead pty swallows them rather than erroring:
    /// typing into a terminal whose shell has exited should do nothing, not
    /// tear anything down.
    pub fn write(&mut self, data: &[u8]) {
        if let Some(pty) = self.pty.as_mut() {
            if pty.write(data).is_err() {
                self.closed = true;
            }
        }
    }

    /// Encode a key press and send it. Scrolled-back output jumps to the bottom
    /// first — typing while reading history should show what you typed.
    pub fn key(&mut self, event: &KeyEvent) {
        let modes = Modes {
            app_cursor: self.vt.screen().app_cursor(),
            app_keypad: self.vt.screen().app_keypad(),
        };
        let bytes = keymap::encode(event, modes);
        if bytes.is_empty() {
            return;
        }
        self.vt.screen_mut().scroll_to_bottom();
        self.vt.screen_mut().selection_clear();
        self.write(&bytes);
    }

    /// Send text as a paste, bracketed when the program asked for that.
    pub fn paste(&mut self, text: &str) {
        let bracketed = self.vt.screen().bracketed_paste();
        let bytes = keymap::encode_paste(text, bracketed);
        self.vt.screen_mut().scroll_to_bottom();
        self.write(&bytes);
    }

    /// Type text as though the user had. For the one thing that needs a shell
    /// rather than a user asking for one: pi's login flow, which exists only
    /// inside pi's own interactive UI.
    pub fn send_text(&mut self, text: &str) {
        let bytes = text.replace("\r\n", "\r").replace('\n', "\r");
        self.write(bytes.as_bytes());
    }

    /// Resize the grid and tell the child. Both halves matter: the engine
    /// reflows what is on screen, and the ioctl is what makes the shell redraw
    /// its prompt at the new width.
    pub fn resize(&mut self, cols: u16, rows: u16, cell_w: u16, cell_h: u16) {
        let grid = Grid::new(cols, rows, cell_w, cell_h);
        self.cell = (grid.cell_w, grid.cell_h);
        if grid.cols as usize == self.vt.screen().cols()
            && grid.rows as usize == self.vt.screen().rows()
        {
            return;
        }
        self.vt
            .screen_mut()
            .resize(grid.cols as usize, grid.rows as usize);
        if let Some(pty) = self.pty.as_ref() {
            pty.resize(grid);
        }
    }

    pub fn font_size(&self) -> i32 {
        self.font_size
    }

    /// Change the point size. The grid does not change here: the UI re-measures
    /// the cell at the new size and calls [`resize`](Self::resize) with what it
    /// found, since only it knows what the glyphs came out as.
    pub fn set_font_size(&mut self, size: i32) {
        self.font_size = sizes::clamp(size);
    }

    pub fn theme_name(&self) -> &str {
        &self.theme_name
    }

    pub fn set_theme(&mut self, name: &str) {
        self.theme_name = name.to_string();
        let theme: &TerminalTheme = terminal_theme(name);
        self.vt.screen_mut().set_theme(theme);
    }

    /// The selected text, or empty when there is no selection.
    pub fn selection_text(&self) -> String {
        self.vt.screen().selection_text()
    }

    /// Stop the shell and reap it. Idempotent, so wiring it to both a tab close
    /// and the window's teardown is safe.
    pub fn shutdown(&mut self) {
        self.stopping.store(true, Ordering::SeqCst);
        if let Some(mut pty) = self.pty.take() {
            crate::debug::proc("terminal.shutdown", &[("pid", pty.child_pid().to_string())]);
            // Closes the master, which breaks the reader out of its blocking
            // read, then signals and reaps the child.
            pty.shutdown();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        self.closed = true;
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn read_loop(fd: libc::c_int, sender: mpsc::Sender<FromPty>, stopping: Arc<AtomicBool>) {
    let mut buf = [0u8; 8192];
    loop {
        if stopping.load(Ordering::SeqCst) {
            break;
        }
        // SAFETY: `fd` is a descriptor this thread owns (a dup made for it),
        // and the buffer is valid for the length passed.
        let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
        match n {
            // A shell exiting closes its end, which reads as EOF here.
            0 => break,
            n if n < 0 => {
                let err = io::Error::last_os_error();
                // A read interrupted by a signal is not a failure; anything
                // else means the pty is gone.
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                break;
            }
            n => {
                if sender
                    .send(FromPty::Output(buf[..n as usize].to_vec()))
                    .is_err()
                {
                    break; // the terminal was dropped
                }
            }
        }
    }
    let _ = sender.send(FromPty::Closed);
    // SAFETY: this thread owns `fd` and is the last to use it.
    unsafe { libc::close(fd) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// A terminal running `cat`, which echoes what it is sent and exits the
    /// moment it is signalled — never an interactive shell in a test.
    fn cat_terminal() -> Terminal {
        let mut command = Command::local_shell(None);
        command.program = "/bin/cat".to_string();
        command.argv = vec!["/bin/cat".to_string()];
        let grid = Grid::new(40, 8, 8, 16);
        let pty = Pty::spawn(&command, grid).expect("spawn cat");
        let (sender, output) = mpsc::channel();
        let stopping = Arc::new(AtomicBool::new(false));
        let fd = pty.dup_master().expect("dup");
        let flag = Arc::clone(&stopping);
        let reader = std::thread::spawn(move || read_loop(fd, sender, flag));
        Terminal {
            vt: Vt::new(40, 8, terminal_theme("dark")),
            pty: Some(pty),
            reader: Some(reader),
            stopping,
            output,
            theme_name: "dark".into(),
            font_size: sizes::DEFAULT_SIZE,
            cell: (8, 16),
            closed: false,
            title: String::new(),
        }
    }

    /// Pump until `check` passes or the deadline runs out. The child answers on
    /// its own schedule, so a test that read once would be a flaky test.
    fn pump_until(terminal: &mut Terminal, check: impl Fn(&mut Terminal) -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            terminal.pump();
            if check(terminal) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    #[test]
    fn what_the_child_writes_reaches_the_screen() {
        let mut terminal = cat_terminal();
        terminal.write(b"hello\r\n");
        assert!(
            pump_until(&mut terminal, |t| t.screen().row_text(0) == "hello"),
            "the echoed text never arrived: {:?}",
            terminal.screen().viewport_text()
        );
        terminal.shutdown();
    }

    #[test]
    fn a_key_press_is_encoded_before_it_is_sent() {
        let mut terminal = cat_terminal();
        // `cat` echoes whatever it receives, so the bytes the keymap produced
        // come back and land on the screen.
        terminal.key(&KeyEvent::new('A' as u32, keymap::mods::NONE, "a"));
        terminal.key(&KeyEvent::new(keymap::key::RETURN, keymap::mods::NONE, "\r"));
        assert!(
            pump_until(&mut terminal, |t| t.screen().row_text(0) == "a"),
            "got {:?}",
            terminal.screen().viewport_text()
        );
        terminal.shutdown();
    }

    #[test]
    fn a_paste_arrives_with_its_newlines_normalised() {
        let mut terminal = cat_terminal();
        terminal.paste("one\ntwo\n");
        assert!(
            pump_until(&mut terminal, |t| t.screen().row_text(1) == "two"),
            "got {:?}",
            terminal.screen().viewport_text()
        );
        terminal.shutdown();
    }

    #[test]
    fn a_shell_that_exits_leaves_its_last_screen_up() {
        let mut terminal = cat_terminal();
        terminal.write(b"final line\r\n");
        assert!(pump_until(&mut terminal, |t| t.screen().row_text(0) == "final line"));
        // Closing the child's input ends `cat`.
        terminal.shutdown();
        terminal.pump();
        assert!(terminal.is_closed());
        assert_eq!(terminal.screen().row_text(0), "final line");
    }

    #[test]
    fn writing_to_a_dead_terminal_is_ignored_rather_than_fatal() {
        let mut terminal = cat_terminal();
        terminal.shutdown();
        terminal.write(b"nobody is listening");
        terminal.key(&KeyEvent::new('A' as u32, keymap::mods::NONE, "a"));
        terminal.paste("still nothing");
        assert!(terminal.is_closed());
    }

    #[test]
    fn shutdown_twice_is_the_same_as_once() {
        let mut terminal = cat_terminal();
        terminal.shutdown();
        terminal.shutdown();
        assert!(terminal.is_closed());
    }

    #[test]
    fn a_resize_moves_the_grid_and_is_a_no_op_when_it_would_not() {
        let mut terminal = cat_terminal();
        terminal.resize(60, 20, 9, 18);
        assert_eq!(terminal.screen().cols(), 60);
        assert_eq!(terminal.screen().rows(), 20);
        assert_eq!(terminal.cell, (9, 18));
        // The same size again changes nothing, but new cell metrics still land:
        // a font-size change resizes the cell without moving the grid.
        terminal.resize(60, 20, 7, 14);
        assert_eq!(terminal.cell, (7, 14));
        terminal.shutdown();
    }

    #[test]
    fn the_font_size_is_held_within_its_bounds() {
        let mut terminal = cat_terminal();
        terminal.set_font_size(1);
        assert_eq!(terminal.font_size(), sizes::MIN_SIZE);
        terminal.set_font_size(500);
        assert_eq!(terminal.font_size(), sizes::MAX_SIZE);
        terminal.set_font_size(11);
        assert_eq!(terminal.font_size(), 11);
        terminal.shutdown();
    }

    #[test]
    fn a_theme_change_repaints_the_same_text_in_the_other_palette() {
        let mut terminal = cat_terminal();
        // Fed straight to the engine: a shell's line discipline would echo the
        // escape back as `^[` and this would be a test of `cat`.
        terminal.feed(b"\x1b[31mred");
        assert_eq!(terminal.screen().row_text(0), "red");
        let before = terminal.render().rows[0].runs[0].fg;
        terminal.set_theme("light");
        assert_eq!(terminal.theme_name(), "light");
        let after = terminal.render().rows[0].runs[0].fg;
        assert_ne!(before, after, "the theme change did not reach the palette");
        terminal.shutdown();
    }

    #[test]
    fn a_title_set_by_the_shell_is_picked_up_once() {
        let mut terminal = cat_terminal();
        assert_eq!(terminal.title(), "");
        terminal.feed(b"\x1b]0;building\x07");
        assert_eq!(terminal.title(), "building");
        // Taken once: a second read must not re-announce a title that has not
        // changed, or every pump would relabel the tab.
        terminal.feed(b"idle");
        assert_eq!(terminal.title(), "building");
        terminal.shutdown();
    }
}
