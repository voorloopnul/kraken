//! The conversation, as QML sees it.
//!
//! One object for the whole pane: the workspace's live sessions, which of them
//! is in front, the transcript that one is writing into, and the attachments
//! the composer is holding. `pupo_core::pi::controller` does the talking to pi
//! and `pupo_core::chat` does the rendering; what is left here is the seam —
//! Qt types out, method calls in, and the cadence that decides when a streaming
//! transcript is worth repainting.
//!
//! Sessions are kept as a pool rather than a single agent, because a running
//! turn must not block starting or opening another conversation: the focused
//! one is what the pane shows, the rest keep streaming behind it. Agents start
//! on the first prompt, so opening a workspace — or glancing at an old session
//! — costs no process at all.
//!
//! There is no QTimer in this binding, so the clock that drives all of it is a
//! QML `Timer` calling [`SessionBridge::pump`]. That is the idiomatic shape
//! here: the event loop belongs to QML, and a bridge that tried to own one
//! would be a second loop running beside it.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use pupo_core::chat::formatting::replay_footers;
use pupo_core::chat::markdown;
use pupo_core::chat::transcript::{thinking_summary, Attachment, Block, FLUSH_DUTY, FLUSH_INTERVAL_MS};
use pupo_core::pi::config;
use pupo_core::pi::controller::{available_levels, RequestKind, SessionController, Signal};
use pupo_core::pi::rpc::{AgentRecord, Launch};
use pupo_core::pi::sessions;
use pupo_core::util::base64;
use pupo_core::{debug, remote};
use qmetaobject::*;
use serde_json::{json, Value};

/// How long Stop waits for the agent to end its turn before offering to kill it
/// outright. Generous, because a working abort still has to unwind an in-flight
/// tool call and close out the turn, and offering to discard a turn that was
/// about to finish on its own is the worse mistake.
const STOP_GRACE: Duration = Duration::from_millis(8000);

/// Image formats a provider accepts in the prompt payload, by the magic bytes
/// that identify them. Sniffed from the content rather than trusted from the
/// extension, which can lie — a JPEG saved as `.png` would otherwise ship
/// mislabeled and be rejected. Anything else travels as a plain path reference.
fn image_mime(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if data.len() > 12 && data.starts_with(b"RIFF") && &data[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

/// What one piece of a rendered reply is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Piece {
    Prose,
    Code,
    /// A thematic break. Split out for the same reason a fence is: the renderer
    /// draws `<hr>` from the widget palette and ignores every colour the markup
    /// asks for, so on a dark theme it comes out as a bright bar. The pane draws
    /// its own hairline instead.
    Rule,
}

impl Piece {
    fn name(self) -> &'static str {
        match self {
            Piece::Prose => "prose",
            Piece::Code => "code",
            Piece::Rule => "rule",
        }
    }
}

/// Split a rendered reply into the prose around its code fences and rules, and
/// those fences and rules themselves, in order.
///
/// The pane lays a card and a floating Copy button over every fence, and the
/// only way to put a QML item on one is to lay the fence out as an item of its
/// own: a `<pre>` buried inside a single rich-text `Text` has no geometry
/// anything outside it can ask for. Splitting the markup is safe because
/// `markdown::render` escapes everything that is not its own output, so the only
/// `<pre>` and `<hr/>` in the string are ones it wrote.
fn segments(html: &str) -> Vec<(Piece, String)> {
    let mut out: Vec<(Piece, String)> = Vec::new();
    let mut rest = html;
    let prose = |out: &mut Vec<(Piece, String)>, text: &str| {
        if !text.is_empty() {
            out.push((Piece::Prose, text.to_string()));
        }
    };
    loop {
        let fence = rest.find("<pre");
        let rule = rest.find("<hr/>");
        let start = match (fence, rule) {
            (Some(a), Some(b)) => a.min(b),
            (Some(a), None) => a,
            (None, Some(b)) => b,
            (None, None) => break,
        };
        let (head, tail) = rest.split_at(start);
        prose(&mut out, head);
        if Some(start) == rule && fence.is_none_or(|at| start < at) {
            out.push((Piece::Rule, String::new()));
            rest = &tail["<hr/>".len()..];
            continue;
        }
        match tail.find("</pre>") {
            Some(end) => {
                let end = end + "</pre>".len();
                out.push((Piece::Code, tail[..end].to_string()));
                rest = &tail[end..];
            }
            // An unterminated fence can only come from a reply still streaming
            // into one; show it as code rather than dropping the tail.
            None => {
                out.push((Piece::Code, tail.to_string()));
                rest = "";
                break;
            }
        }
    }
    prose(&mut out, rest);
    out
}

/// A file the composer is holding for the next prompt.
struct Pending {
    /// What the chip shows.
    chip: Attachment,
    /// The prompt-ready `{type, data, mimeType}` object for an image, or `None`
    /// for a file that rides along as a path reference in the message text —
    /// pi's prompt carries images structurally but has no file channel.
    image: Option<Value>,
}

/// One live session, and the pane state that belongs to it rather than to the
/// pane.
///
/// The stop escalation is tracked per session for the same reason the busy row
/// is: switching away and back has to show the state of the session in front of
/// you, not the one that happened to stall.
struct Live {
    controller: SessionController,
    /// Title and model read straight off a session file when one is opened
    /// from History. The controller learns both from the agent, but the agent
    /// does not start until the first prompt — without these a browsed session
    /// would sit untitled under a blank model pill.
    stored_title: String,
    stored_model: String,
    /// Set when an abort has been sent and is being given its grace; cleared
    /// when the turn ends however it ends.
    stop_deadline: Option<Instant>,
    /// The abort never landed, so the button now offers to kill pi.
    stop_armed: bool,
    /// A turn finished here while you were reading another conversation. The
    /// dot on its History row is the whole point of the flag; it is cleared the
    /// moment the session comes to the front.
    unseen: bool,
    /// What History calls this session. Its file path once pi has named one,
    /// and a made-up id until then — a session in its first turn has no file
    /// yet, and a row that could not be clicked back to would strand the turn
    /// that is running in it.
    key: String,
}

impl Live {
    fn new(controller: SessionController) -> Self {
        Self {
            controller,
            stored_title: String::new(),
            stored_model: String::new(),
            stop_deadline: None,
            stop_armed: false,
            unseen: false,
            key: String::new(),
        }
    }

    /// The key History knows it by: its file once it has one, so a live row and
    /// the disk row for the same session collapse into one.
    fn key(&self) -> &str {
        self.controller.session_path().unwrap_or(&self.key)
    }

    fn title(&self) -> &str {
        match self.controller.title() {
            "" => self.stored_title.as_str(),
            live => live,
        }
    }

    fn model_label(&self) -> &str {
        self.controller
            .model_label()
            .unwrap_or(self.stored_model.as_str())
    }
}

#[derive(QObject, Default)]
pub struct SessionBridge {
    base: qt_base_class!(trait QObject),

    /// The focused transcript, one map per block: always `kind` and `index`,
    /// then whatever that kind carries. Rebuilt whole on every repaint — a list
    /// that is replaced cannot disagree with itself, and the flush cadence
    /// below is what keeps that from happening on every streamed token.
    blocks: qt_property!(QVariantList; NOTIFY blocks_changed READ get_blocks),
    /// Whether the focused session has a turn in flight.
    busy: qt_property!(bool; NOTIFY status_changed READ get_busy),
    /// When the current turn began, in the epoch milliseconds `Date.now()`
    /// speaks, or 0 when nothing is running.
    ///
    /// A start rather than an elapsed count: the busy row's clock ticks once a
    /// second, and a property that carried the elapsed time would have to
    /// notify at that rate for a number QML can work out for itself. It is the
    /// turn's own start, so switching to an already-running session shows its
    /// true elapsed rather than restarting from zero.
    busy_since: qt_property!(f64; NOTIFY status_changed READ get_busy_since),
    /// Whether pi is summarizing history away rather than answering.
    compacting: qt_property!(bool; NOTIFY status_changed READ get_compacting),
    /// How full the model's context window is, or "" when there is no honest
    /// number to show.
    context_label: qt_property!(QString; NOTIFY status_changed READ get_context_label),
    /// Whether Stop has been pressed and never landed, so the button is now
    /// offering to kill pi outright.
    stop_armed: qt_property!(bool; NOTIFY status_changed READ get_stop_armed),
    /// The composer's model pill.
    model_label: qt_property!(QString; NOTIFY footer_changed READ get_model_label),
    /// The composer's effort pill, or "" while the level is still unknown.
    effort_label: qt_property!(QString; NOTIFY footer_changed READ get_effort_label),
    /// False only once the model has *confirmed* it has no thinking levels, so
    /// the pill hides rather than sitting on a placeholder forever.
    effort_supported: qt_property!(bool; NOTIFY footer_changed READ get_effort_supported),
    /// The levels the effort popup lists.
    effort_levels: qt_property!(QVariantList; NOTIFY footer_changed READ get_effort_levels),
    /// The model picker's list: `{ provider, id, name, current }` per entry,
    /// narrowed to the scope chosen in Settings › Models.
    models: qt_property!(QVariantList; NOTIFY models_changed READ get_models),
    /// The composer's attachment chips: `{ name, path, preview }` per entry.
    attachments: qt_property!(QVariantList; NOTIFY attachments_changed READ get_attachments),
    /// The focused session's title — the first thing the user said in it.
    title: qt_property!(QString; NOTIFY title_changed READ get_title),
    /// Every session this workspace is holding: `{ key, title, path, running,
    /// unseen }`. History shows them beside the ones on disk, so a turn that is
    /// streaming right now is reachable before pi has written a file for it.
    ///
    /// Published rather than pushed: this object has one owner and no opinion
    /// about which other objects exist, so the workspace view is what carries
    /// the list across (see WorkspaceView.qml).
    live_sessions: qt_property!(QVariantList; NOTIFY live_changed READ get_live_sessions),
    /// The focused session's key, so History can mark the row that is open.
    focused_key: qt_property!(QString; NOTIFY live_changed READ get_focused_key),

    blocks_changed: qt_signal!(),
    live_changed: qt_signal!(),
    status_changed: qt_signal!(),
    footer_changed: qt_signal!(),
    models_changed: qt_signal!(),
    attachments_changed: qt_signal!(),
    title_changed: qt_signal!(),
    /// A link in the transcript was clicked. The browser panel belongs to
    /// another object, so this says what was asked for and lets the workspace
    /// route it.
    link_activated: qt_signal!(url: QString),
    /// A model list asked for by [`request_models`](Self::request_models) has
    /// arrived — the picker opens on this rather than on the click, since the
    /// fetch is a round trip to the agent. An empty `models` means the fetch
    /// failed or nothing is configured, which the pane says rather than opening
    /// an empty menu.
    models_ready: qt_signal!(),
    /// The same, for the effort popup's levels.
    effort_ready: qt_signal!(),

    /// Drain every agent and repaint if anything changed. Called from a QML
    /// `Timer`; see the module docs for why the clock lives there.
    pump: qt_method!(fn(&mut self)),
    /// Point the pane at a workspace. Switching retires the sessions of the one
    /// being left — their agents belong to that folder — and opens a fresh
    /// empty session ready to type in.
    set_workspace: qt_method!(fn(&mut self, path: QString)),
    /// The theme and base size the transcript renders at. Pushed in from QML
    /// rather than read from `Theme`, so this object has one owner and no
    /// opinion about which other objects exist.
    set_theme: qt_method!(fn(&mut self, name: QString)),
    set_font_size: qt_method!(fn(&mut self, size: i32)),

    submit: qt_method!(fn(&mut self, text: QString, files: QVariantList)),
    /// The Stop button. The first press aborts; once the grace has run out and
    /// the button has re-armed, it kills pi instead.
    stop: qt_method!(fn(&mut self)),
    force_stop: qt_method!(fn(&mut self)),
    new_session: qt_method!(fn(&mut self)),
    load_session: qt_method!(fn(&mut self, path: QString)),
    /// Bring a session that is already live to the front, by its key. A row
    /// History drew from `live_sessions` has no file to open, so opening it is
    /// a focus rather than a load.
    focus_session: qt_method!(fn(&mut self, key: QString)),
    set_expanded: qt_method!(fn(&mut self, index: i32, open: bool)),
    /// The source of one code block, by its running index in the transcript.
    ///
    /// Returned rather than put on the clipboard here: this binding cannot
    /// reach `QGuiApplication::clipboard`, so the pane does the copying (see
    /// `Clipboard.qml`) and this answers what to copy.
    copy_code: qt_method!(fn(&self, index: i32) -> QString),
    request_models: qt_method!(fn(&mut self)),
    set_model: qt_method!(fn(&mut self, provider: QString, id: QString)),
    request_effort: qt_method!(fn(&mut self)),
    set_effort: qt_method!(fn(&mut self, level: QString)),
    attach_file: qt_method!(fn(&mut self, path: QString)),
    remove_attachment: qt_method!(fn(&mut self, index: i32)),
    /// Route a transcript link to whoever shows web pages.
    open_link: qt_method!(fn(&mut self, url: QString)),
    /// Reap every agent. The window closing is the one caller.
    shutdown: qt_method!(fn(&mut self)),

    sessions: Vec<Live>,
    focused: usize,
    workspace: String,
    theme_name: String,
    font_size: i32,

    /// The composer's held attachments, chips and payloads together.
    pending: Vec<Pending>,
    /// The picker's list as the agent last reported it.
    model_list: Vec<Value>,
    /// Set while a click is waiting on the fetch behind it, so a list that
    /// arrives for a session nobody is looking at any more opens nothing.
    models_wanted: bool,
    effort_wanted: bool,
    /// Numbers the made-up keys a session wears until pi names its file.
    next_key: u64,
    /// Request ids whose reply carries the model list. The controller ignores
    /// those replies by design — the picker is the owner's business — so they
    /// are recognised here on the way past.
    model_requests: Vec<String>,

    /// The last repaint, and what the next one may cost before it is pushed
    /// out. A paint is not free, and an expanded reasoning block re-parses its
    /// whole markdown; without the second rule a long one would spend the
    /// entire frame budget repainting and stop answering the mouse.
    painted_at: Option<Instant>,
    paint_interval: u64,
    /// A streamed change waiting for the cadence above.
    paint_pending: bool,

    /// The rendered blocks, held between repaints so the property getter is a
    /// copy rather than a re-render: QML reads it once per notify, but a
    /// binding that read it twice would otherwise render the transcript twice.
    painted: QVariantList,
    /// Every code fence in the painted transcript, in order, for `copy_code`.
    code_sources: Vec<String>,
}

impl SessionBridge {
    pub fn new() -> Self {
        Self {
            theme_name: pupo_core::theme::DEFAULT_THEME.to_string(),
            font_size: pupo_core::typography::chat::DEFAULT_SIZE,
            paint_interval: FLUSH_INTERVAL_MS,
            ..Default::default()
        }
    }

    // ---- Session pool ------------------------------------------------------

    fn current(&self) -> Option<&Live> {
        self.sessions.get(self.focused)
    }

    fn current_mut(&mut self) -> Option<&mut Live> {
        self.sessions.get_mut(self.focused)
    }

    fn launch(&self, session_path: Option<&str>) -> Launch {
        let mut launch = Launch::new(&self.workspace);
        if let Some(path) = session_path {
            launch = launch.session(path);
        }
        // A remote workspace runs pi locally in the anchor folder and routes
        // its tools over SSH; `resolve` is what knows which folders those are.
        match remote::resolve(&self.workspace) {
            Some(target) => launch.remote(target),
            None => launch,
        }
    }

    /// Open a session and put it in front, retiring the one being left unless
    /// it is still streaming — then it stays live in the background so you can
    /// flip back and watch it.
    fn focus_new(&mut self, session_path: Option<&str>) -> usize {
        let controller = SessionController::new(self.launch(session_path));
        let previous = self.focused;
        let mut live = Live::new(controller);
        // A key of its own until pi names a file, so a session that is streaming
        // before it has been written is still a row you can click back to.
        self.next_key += 1;
        live.key = format!("live-{}", self.next_key);
        self.sessions.push(live);
        self.focused = self.sessions.len() - 1;
        if let Some(live) = self.sessions.get(previous) {
            if previous != self.focused && !live.controller.is_streaming() {
                self.sessions.remove(previous);
                self.focused -= 1;
            }
        }
        self.focused
    }

    /// Everything about the focused session that the pane shows at once.
    fn announce_all(&mut self) {
        // Coming to the front is what makes a finished turn seen.
        if let Some(live) = self.current_mut() {
            live.unseen = false;
        }
        self.repaint();
        self.status_changed();
        self.footer_changed();
        self.title_changed();
        self.live_changed();
    }

    fn set_workspace(&mut self, path: QString) {
        let path = path.to_string();
        if path == self.workspace {
            return;
        }
        for live in &mut self.sessions {
            live.controller.shutdown();
        }
        self.sessions.clear();
        self.focused = 0;
        self.workspace = path;
        self.pending.clear();
        self.model_list.clear();
        if !self.workspace.is_empty() {
            self.focus_new(None);
        }
        self.attachments_changed();
        self.models_changed();
        self.announce_all();
    }

    fn set_theme(&mut self, name: QString) {
        let name = name.to_string();
        if name != self.theme_name {
            self.theme_name = name;
            self.repaint();
        }
    }

    fn set_font_size(&mut self, size: i32) {
        let size = pupo_core::typography::chat::clamp(size);
        if size != self.font_size {
            self.font_size = size;
            self.repaint();
        }
    }

    fn new_session(&mut self) {
        // Reuse the current session when it is already a fresh, unused one:
        // pressing New twice should not leave a trail of empty conversations.
        if let Some(live) = self.current() {
            if !live.controller.is_streaming()
                && live.controller.session_path().is_none()
                && live.controller.transcript.is_empty()
            {
                return;
            }
        }
        debug::action("session.new", &[]);
        self.focus_new(None);
        self.announce_all();
    }

    fn load_session(&mut self, path: QString) {
        let path = path.to_string();
        // A path with no file behind it opens nothing. Anything else would
        // start an agent on a session that does not exist and leave an empty
        // conversation standing where the reader asked for a real one.
        if path.is_empty() || !std::path::Path::new(&path).is_file() {
            return;
        }
        // Already live — running, or in flight and maybe not yet on disk. Jump
        // straight to it without touching its agent.
        if let Some(index) = self
            .sessions
            .iter()
            .position(|live| live.controller.session_path() == Some(path.as_str()))
        {
            self.focused = index;
            self.announce_all();
            return;
        }
        debug::action("session.open", &[("path", path.clone())]);
        let index = self.focus_new(Some(&path));
        // Read the file rather than asking a pi that is not running: pi writes
        // the same message objects to disk that it returns over RPC, so this
        // renders identically while costing nothing. Browsing a past session
        // stays free until you actually continue it.
        let stored = sessions::read_transcript(&path);
        let footers: Vec<(usize, String)> =
            replay_footers(&stored.messages, &stored.written)
                .into_iter()
                .collect();
        let live = &mut self.sessions[index];
        live.controller
            .transcript
            .render_messages(&stored.messages, &footers);
        live.stored_title = stored
            .messages
            .iter()
            .find(|m| m.get("role").and_then(Value::as_str) == Some("user"))
            .map(|m| {
                pupo_core::chat::formatting::content_text(
                    m.get("content").unwrap_or(&Value::Null),
                )
            })
            .unwrap_or_default();
        // There is no friendly name on disk, so the pill shows the id until the
        // agent later resolves it.
        live.stored_model = stored
            .model
            .as_ref()
            .and_then(|model| model.get("id"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let _ = live.controller.transcript.take_dirty();
        self.announce_all();
    }

    fn focus_session(&mut self, key: QString) {
        let key = key.to_string();
        let Some(index) = self.sessions.iter().position(|live| live.key() == key) else {
            return;
        };
        if index == self.focused {
            return;
        }
        self.focused = index;
        self.announce_all();
    }

    fn shutdown(&mut self) {
        for live in &mut self.sessions {
            live.controller.shutdown();
        }
        self.sessions.clear();
    }

    fn get_live_sessions(&self) -> QVariantList {
        let mut list = QVariantList::default();
        for (index, live) in self.sessions.iter().enumerate() {
            // A session nobody has said anything in yet is not history. It is
            // the empty pane you are looking at, and a row for it would sit
            // above the real conversations saying nothing.
            if live.controller.transcript.is_empty()
                && live.controller.session_path().is_none()
                && !live.controller.is_streaming()
            {
                continue;
            }
            let mut map = QVariantMap::default();
            map.insert("key".into(), text(live.key()));
            map.insert("title".into(), text(live.title()));
            map.insert(
                "path".into(),
                text(live.controller.session_path().unwrap_or_default()),
            );
            map.insert(
                "running".into(),
                QVariant::from(live.controller.is_streaming()),
            );
            map.insert("unseen".into(), QVariant::from(live.unseen));
            map.insert("current".into(), QVariant::from(index == self.focused));
            list.push(map.into());
        }
        list
    }

    fn get_focused_key(&self) -> QString {
        self.current().map(Live::key).unwrap_or_default().into()
    }

    // ---- The clock ---------------------------------------------------------

    fn pump(&mut self) {
        let mut status = false;
        let mut footer = false;
        let mut title = false;
        let mut dirty = false;
        let mut live = false;

        for index in 0..self.sessions.len() {
            // Drained here rather than through `SessionController::pump`, so
            // the model list can be lifted out of the reply on its way past:
            // the controller ignores that reply by design, since what a picker
            // does with it is the owner's business.
            let records = self.sessions[index].controller.agent.drain();
            for record in records {
                if let AgentRecord::Response { id, value } = &record {
                    if let Some(at) = self.model_requests.iter().position(|known| known == id) {
                        self.model_requests.remove(at);
                        self.model_list = SessionController::models_from(value);
                        if self.models_wanted && index == self.focused {
                            self.models_wanted = false;
                            self.models_changed();
                            self.models_ready();
                        }
                    }
                }
                self.sessions[index].controller.on_record(record);
            }
            for signal in self.sessions[index].controller.take_signals() {
                let focused = index == self.focused;
                match signal {
                    Signal::TranscriptChanged { streaming } => {
                        if focused {
                            dirty = true;
                            if !streaming {
                                self.paint_pending = true;
                                self.painted_at = None; // structural: paint now
                            }
                        }
                    }
                    Signal::StreamingChanged(streaming) => {
                        if !streaming {
                            // However the turn ended, any pending or offered
                            // force stop is moot.
                            self.sessions[index].stop_deadline = None;
                            self.sessions[index].stop_armed = false;
                            // A turn that finished behind your back leaves a
                            // mark; one you watched finish does not.
                            self.sessions[index].unseen = !focused;
                        }
                        status |= focused;
                        live = true;
                    }
                    Signal::CompactingChanged(_) | Signal::ContextKnown(_) => {
                        status |= focused;
                    }
                    Signal::ModelKnown(_) | Signal::ThinkingKnown(_) => {
                        footer |= focused;
                        if self.effort_wanted && focused {
                            self.effort_wanted = false;
                            self.effort_ready();
                        }
                    }
                    Signal::TitleKnown(_) => {
                        title |= focused;
                        live = true;
                    }
                    // pi has named the session's file, which is the key History
                    // knows it by from now on.
                    Signal::PathKnown(_) => live = true,
                }
            }
            self.send_requests(index);
            // A stop that never landed. The distinction is the point: an abort
            // that is merely slow looks exactly like one that will never land,
            // so the row says which it is rather than leaving the reader to
            // guess from a climbing clock.
            let expired = self.sessions[index]
                .stop_deadline
                .is_some_and(|at| at.elapsed() >= STOP_GRACE);
            if expired {
                self.sessions[index].stop_deadline = None;
                if self.sessions[index].controller.is_streaming() {
                    self.sessions[index].stop_armed = true;
                    status |= index == self.focused;
                }
            }
        }

        if dirty {
            self.paint_pending = true;
        }
        if self.paint_pending && self.paint_due() {
            self.repaint();
        }
        if status {
            self.status_changed();
        }
        if footer {
            self.footer_changed();
        }
        if title {
            self.title_changed();
        }
        if live {
            self.live_changed();
        }
    }

    /// Whether the cadence allows a repaint yet. A structural change clears the
    /// last-painted mark, which is what lets a tool call or a footer land at
    /// once — those arrive at human speed, and waiting on one would look like a
    /// stall.
    fn paint_due(&self) -> bool {
        match self.painted_at {
            None => true,
            Some(at) => at.elapsed() >= Duration::from_millis(self.paint_interval),
        }
    }

    /// Send what a controller asked for and tell it which reply is which.
    fn send_requests(&mut self, index: usize) {
        for request in self.sessions[index].controller.take_requests() {
            let kind = request.kind;
            match self.sessions[index].controller.agent.request(request.command) {
                Ok(id) => {
                    if kind == RequestKind::Models {
                        self.model_requests.push(id.clone());
                    }
                    self.sessions[index].controller.register(&id, kind);
                }
                Err(error) => debug::error("agent.request", &[("error", error.to_string())]),
            }
        }
    }

    // ---- Painting ----------------------------------------------------------

    /// Rebuild the painted transcript and tell QML to re-read it.
    fn repaint(&mut self) {
        let started = Instant::now();
        let (list, sources) = self.paint_blocks();
        self.painted = list;
        self.code_sources = sources;
        self.paint_pending = false;
        self.painted_at = Some(Instant::now());
        // Charge the next interval for what this one cost: re-rendering a long
        // expanded block is O(block), and a paint allowed to run back to back
        // would eat the whole frame budget.
        let cost = started.elapsed().as_millis() as u64;
        self.paint_interval = FLUSH_INTERVAL_MS.max(cost * u64::from(FLUSH_DUTY));
        self.blocks_changed();
    }

    fn paint_blocks(&self) -> (QVariantList, Vec<String>) {
        let mut list = QVariantList::default();
        let mut sources: Vec<String> = Vec::new();
        let Some(live) = self.current() else {
            return (list, sources);
        };
        for (index, block) in live.controller.transcript.blocks().iter().enumerate() {
            let mut map = QVariantMap::default();
            map.insert("kind".into(), text(block.kind()));
            map.insert("index".into(), QVariant::from(index as i32));
            match block {
                Block::User {
                    text: body,
                    attachments,
                } => {
                    map.insert("text".into(), text(body));
                    map.insert("attachments".into(), chips(attachments).into());
                }
                Block::Assistant { markdown } => {
                    let rendered = markdown::render(markdown, &self.theme_name, self.font_size);
                    let mut parts = QVariantList::default();
                    let mut fence = 0usize;
                    for (piece, html) in segments(&rendered.html) {
                        let is_code = piece == Piece::Code;
                        let mut part = QVariantMap::default();
                        part.insert("kind".into(), text(piece.name()));
                        part.insert("html".into(), text(&html));
                        // The running index a Copy button hands back, and the
                        // language for the card's caption.
                        let (source, language) = match rendered.code_blocks.get(fence) {
                            Some(block) if is_code => {
                                (block.source.as_str(), block.language.as_str())
                            }
                            _ => ("", ""),
                        };
                        part.insert(
                            "source".into(),
                            QVariant::from(if is_code {
                                sources.push(source.to_string());
                                fence += 1;
                                sources.len() as i32 - 1
                            } else {
                                -1
                            }),
                        );
                        part.insert("language".into(), text(language));
                        parts.push(part.into());
                    }
                    map.insert("segments".into(), parts.into());
                }
                Block::Thinking { text: body, expanded } => {
                    map.insert("expanded".into(), QVariant::from(*expanded));
                    map.insert("summary".into(), text(&thinking_summary(body)));
                    // Only rendered when it is on screen: the summary above is
                    // what a collapsed row shows, and parsing the whole
                    // reasoning to build a line nobody reads is what used to
                    // lock the window up mid-stream.
                    let html = if *expanded {
                        markdown::render(body, &self.theme_name, self.font_size).html
                    } else {
                        String::new()
                    };
                    map.insert("html".into(), text(&html));
                }
                Block::Tool {
                    name,
                    summary,
                    detail,
                    expanded,
                } => {
                    map.insert("name".into(), text(name));
                    map.insert("summary".into(), text(summary));
                    map.insert("detail".into(), text(if *expanded { detail } else { "" }));
                    map.insert("expanded".into(), QVariant::from(*expanded));
                    map.insert("has_detail".into(), QVariant::from(!detail.is_empty()));
                }
                Block::Info { text: body, error } => {
                    map.insert("text".into(), text(body));
                    map.insert("error".into(), QVariant::from(*error));
                }
                Block::Footer { text: body } => {
                    map.insert("text".into(), text(body));
                }
            }
            list.push(map.into());
        }
        (list, sources)
    }

    fn get_blocks(&self) -> QVariantList {
        self.painted.clone()
    }

    fn set_expanded(&mut self, index: i32, open: bool) {
        let Ok(index) = usize::try_from(index) else {
            return;
        };
        let changed = self
            .current_mut()
            .is_some_and(|live| live.controller.transcript.set_expanded(index, open));
        if changed {
            if let Some(live) = self.current_mut() {
                let _ = live.controller.transcript.take_dirty();
            }
            self.repaint();
        }
    }

    fn copy_code(&self, index: i32) -> QString {
        usize::try_from(index)
            .ok()
            .and_then(|index| self.code_sources.get(index))
            .map(String::as_str)
            .unwrap_or_default()
            .into()
    }

    fn open_link(&mut self, url: QString) {
        self.link_activated(url);
    }

    // ---- Status ------------------------------------------------------------

    fn get_busy(&self) -> bool {
        self.current().is_some_and(|live| live.controller.is_streaming())
    }

    fn get_busy_since(&self) -> f64 {
        // Wall-clock, because QML measures the elapsed time against
        // `Date.now()`. The controller counts from a monotonic instant, so the
        // start is derived by subtracting how long it has been running rather
        // than remembered as a timestamp that a clock change could move.
        let Some(seconds) = self
            .current()
            .and_then(|live| {
                live.controller
                    .streaming_for()
                    .or_else(|| live.controller.compacting_for())
            })
        else {
            return 0.0;
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|since| since.as_secs_f64())
            .unwrap_or_default();
        (now - seconds) * 1000.0
    }

    fn get_compacting(&self) -> bool {
        self.current().is_some_and(|live| live.controller.is_compacting())
    }

    fn get_context_label(&self) -> QString {
        self.current()
            .and_then(|live| live.controller.context_label())
            .unwrap_or_default()
            .into()
    }

    fn get_stop_armed(&self) -> bool {
        self.current().is_some_and(|live| live.stop_armed)
    }

    fn get_title(&self) -> QString {
        self.current().map(Live::title).unwrap_or_default().into()
    }

    fn get_model_label(&self) -> QString {
        self.current().map(Live::model_label).unwrap_or_default().into()
    }

    fn get_effort_label(&self) -> QString {
        self.current()
            .and_then(|live| live.controller.thinking_label())
            .unwrap_or_default()
            .into()
    }

    fn get_effort_supported(&self) -> bool {
        // Unknown still shows the pill on its placeholder; only a confirmed
        // empty list hides it, since a model whose levels have not arrived yet
        // is not a model without any.
        self.current().is_some_and(|live| {
            live.controller.thinking_label().is_some()
                || live.controller.thinking_levels().is_empty()
                    && live.controller.model_label().is_none()
        })
    }

    fn get_effort_levels(&self) -> QVariantList {
        let mut list = QVariantList::default();
        if let Some(live) = self.current() {
            for level in live.controller.thinking_levels() {
                list.push(text(level));
            }
        }
        list
    }

    // ---- Stopping ----------------------------------------------------------

    fn stop(&mut self) {
        let armed = self.current().is_some_and(|live| live.stop_armed);
        if armed {
            self.force_stop();
            return;
        }
        debug::action("chat.stop", &[]);
        if let Some(live) = self.current_mut() {
            live.controller.stop();
            // An abort that works clears the busy row in a second or two. When
            // it doesn't, the turn is wedged somewhere no message reaches and
            // only killing pi will end it — so start the clock, and if it runs
            // out say so instead of leaving a dead button under a climbing
            // timer.
            live.stop_deadline = Some(Instant::now());
        }
    }

    fn force_stop(&mut self) {
        debug::action("chat.force-stop", &[]);
        if let Some(live) = self.current_mut() {
            live.stop_deadline = None;
            live.stop_armed = false;
            live.controller.force_stop();
            let _ = live.controller.transcript.take_dirty();
        }
        self.repaint();
        self.status_changed();
    }

    // ---- Composer ----------------------------------------------------------

    fn submit(&mut self, text: QString, files: QVariantList) {
        let text = text.to_string();
        let text = text.trim().to_string();
        // Files dropped straight onto the composer at send time join whatever
        // the chips are already holding.
        for index in 0..files.len() {
            self.attach_path(&local_path(&files[index].to_qstring().to_string()));
        }
        if text.is_empty() && self.pending.is_empty() {
            return;
        }
        if self.sessions.is_empty() {
            if self.workspace.is_empty() {
                return;
            }
            self.focus_new(None);
        }
        let images: Vec<Value> = self
            .pending
            .iter()
            .filter_map(|held| held.image.clone())
            .collect();
        let paths: Vec<String> = self
            .pending
            .iter()
            .filter(|held| held.image.is_none())
            .map(|held| held.chip.path.clone())
            .collect();
        debug::action(
            "chat.submit",
            &[
                ("chars", text.chars().count().to_string()),
                ("images", images.len().to_string()),
                ("files", paths.len().to_string()),
            ],
        );
        let index = self.focused;
        let wire = self.sessions[index]
            .controller
            .prompt(&text, &images, &paths);
        match self.sessions[index].controller.agent.prompt(&wire, &images) {
            Ok(id) => self.sessions[index]
                .controller
                .register(&id, RequestKind::Ack("Prompt rejected")),
            Err(error) => {
                self.sessions[index]
                    .controller
                    .transcript
                    .add_info(&format!("Pi agent unavailable: {error}"), true);
            }
        }
        self.send_requests(index);
        self.pending.clear();
        let _ = self.sessions[index].controller.transcript.take_dirty();
        self.attachments_changed();
        self.announce_all();
    }

    fn attach_file(&mut self, path: QString) {
        self.attach_path(&local_path(&path.to_string()));
        self.attachments_changed();
    }

    fn attach_path(&mut self, path: &str) {
        if path.is_empty() {
            return;
        }
        let file = std::path::Path::new(path);
        let name = file
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string());
        // Read once: the same bytes decide whether this is an image and, if it
        // is, become the payload. Unreadable — vanished between the dialog and
        // now — still attaches, as a path reference rather than a crash.
        let data = std::fs::read(file).unwrap_or_default();
        let image = image_mime(&data).map(|mime| {
            json!({ "type": "image", "data": base64(&data), "mimeType": mime })
        });
        // The thumbnail comes off disk rather than out of a `data:` URL: the
        // file is right there, and a base64 copy of a photograph in a property
        // is a megabyte QML has to re-parse on every read.
        let preview = match image {
            Some(_) => format!("file://{path}"),
            None => String::new(),
        };
        self.pending.push(Pending {
            chip: Attachment {
                name,
                path: path.to_string(),
                preview,
            },
            image,
        });
    }

    fn remove_attachment(&mut self, index: i32) {
        let Ok(index) = usize::try_from(index) else {
            return;
        };
        if index < self.pending.len() {
            self.pending.remove(index);
            self.attachments_changed();
        }
    }

    fn get_attachments(&self) -> QVariantList {
        let held: Vec<Attachment> = self.pending.iter().map(|p| p.chip.clone()).collect();
        chips(&held)
    }

    // ---- Model and effort --------------------------------------------------

    fn request_models(&mut self) {
        self.models_wanted = true;
        let index = self.focused;
        if index >= self.sessions.len() {
            return;
        }
        self.sessions[index].controller.request_models();
        self.send_requests(index);
    }

    fn set_model(&mut self, provider: QString, id: QString) {
        let (provider, id) = (provider.to_string(), id.to_string());
        debug::action(
            "model.select",
            &[("provider", provider.clone()), ("model", id.clone())],
        );
        let index = self.focused;
        if index >= self.sessions.len() {
            return;
        }
        self.sessions[index].controller.set_model(&provider, &id);
        self.send_requests(index);
        self.footer_changed();
    }

    fn request_effort(&mut self) {
        let index = self.focused;
        if index >= self.sessions.len() {
            return;
        }
        // Already known: the levels came in with an earlier state fetch, and
        // making the reader wait for a round trip they do not need is worse
        // than opening on what is already true.
        if !self.sessions[index].controller.thinking_levels().is_empty() {
            self.effort_ready();
            return;
        }
        self.effort_wanted = true;
        self.sessions[index].controller.sync_state();
        self.send_requests(index);
    }

    fn set_effort(&mut self, level: QString) {
        let level = level.to_string();
        debug::action("effort.select", &[("level", level.clone())]);
        let index = self.focused;
        if index >= self.sessions.len() {
            return;
        }
        self.sessions[index].controller.set_thinking_level(&level);
        self.send_requests(index);
        self.footer_changed();
    }

    /// The models the picker shows: pi's list narrowed to the scope chosen in
    /// Settings › Models, with the session's own model kept on it whatever the
    /// scope says. It is already running, the picker marks it as current, and
    /// dropping it would leave the one model you are using as the one you
    /// cannot switch back to.
    fn get_models(&self) -> QVariantList {
        let current = self.current().and_then(|live| live.controller.model_label());
        let mut offered = config::in_scope(&self.model_list, None);
        let known = |model: &Value| {
            model.get("id").and_then(Value::as_str) == current && current.is_some()
        };
        if !offered.iter().any(known) {
            let mut kept: Vec<Value> =
                self.model_list.iter().filter(|m| known(m)).cloned().collect();
            kept.append(&mut offered);
            offered = kept;
        }
        let mut list = QVariantList::default();
        for model in &offered {
            let field = |key: &str| {
                model
                    .get(key)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            };
            let mut map = QVariantMap::default();
            let id = field("id");
            map.insert("provider".into(), text(&field("provider")));
            map.insert("id".into(), text(&id));
            map.insert("name".into(), text(&field("name")));
            map.insert("levels".into(), QVariant::from(!available_levels(model).is_empty()));
            map.insert("current".into(), QVariant::from(Some(id.as_str()) == current));
            list.push(map.into());
        }
        list
    }
}

/// One attachment row per chip, as QML reads them.
fn chips(attachments: &[Attachment]) -> QVariantList {
    let mut list = QVariantList::default();
    for attachment in attachments {
        let mut map = QVariantMap::default();
        map.insert("name".into(), text(&attachment.name));
        map.insert("path".into(), text(&attachment.path));
        map.insert("preview".into(), text(&attachment.preview));
        list.push(map.into());
    }
    list
}

fn text(value: &str) -> QVariant {
    QVariant::from(QString::from(value))
}

/// A path out of whatever QML handed over: a `FileDialog` and a `DropArea` both
/// speak `file://` URLs, while an explicit call speaks plain paths.
fn local_path(value: &str) -> String {
    match value.strip_prefix("file://") {
        Some(path) => path.to_string(),
        None => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reply_splits_into_prose_and_its_fences() {
        let rendered = markdown::render("hi\n\n```rs\nlet x = 1;\n```\n\nbye\n", "dark", 13);
        let parts = segments(&rendered.html);
        assert_eq!(parts.len(), 3);
        assert!(parts[0].0 == Piece::Prose && parts[0].1.contains("hi"));
        assert!(parts[1].0 == Piece::Code
            && parts[1].1.starts_with("<pre")
            && parts[1].1.ends_with("</pre>"));
        assert!(parts[2].0 == Piece::Prose && parts[2].1.contains("bye"));
    }

    #[test]
    fn prose_with_no_fence_is_one_segment() {
        let parts = segments("<p>just words</p>");
        assert_eq!(parts, vec![(Piece::Prose, "<p>just words</p>".to_string())]);
    }

    #[test]
    fn a_thematic_break_is_a_piece_of_its_own() {
        let parts = segments("<p>a</p><hr/><p>b</p>");
        assert_eq!(
            parts,
            vec![
                (Piece::Prose, "<p>a</p>".to_string()),
                (Piece::Rule, String::new()),
                (Piece::Prose, "<p>b</p>".to_string()),
            ]
        );
    }

    #[test]
    fn a_rule_inside_a_fence_is_left_in_the_fence() {
        // The renderer escapes everything it did not write, so this can only be
        // a rule of its own — but the split still has to pick the earlier of the
        // two markers rather than whichever it looked for first.
        let parts = segments("<pre>x</pre><hr/>");
        assert_eq!(
            parts,
            vec![
                (Piece::Code, "<pre>x</pre>".to_string()),
                (Piece::Rule, String::new()),
            ]
        );
    }

    #[test]
    fn an_image_is_recognised_by_its_bytes_not_its_name() {
        assert_eq!(image_mime(b"\x89PNG\r\n\x1a\n rest"), Some("image/png"));
        assert_eq!(image_mime(b"RIFF____WEBPVP8 "), Some("image/webp"));
        assert_eq!(image_mime(b"not an image"), None);
    }

    #[test]
    fn a_dropped_url_becomes_a_path() {
        assert_eq!(local_path("file:///tmp/a.png"), "/tmp/a.png");
        assert_eq!(local_path("/tmp/a.png"), "/tmp/a.png");
    }
}
