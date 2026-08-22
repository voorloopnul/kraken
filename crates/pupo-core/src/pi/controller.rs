//! One session: the agent, the transcript it writes into, and the state
//! machine between them.
//!
//! The controller is where a stream of Pi events becomes something a reader can
//! follow. It owns no UI at all — every observable change leaves here as a
//! [`Signal`], and the transcript it keeps is [`crate::chat::transcript`]'s
//! model rather than anything painted. That is what makes a turn's whole
//! lifecycle testable: feed it the events pi would have sent and assert on the
//! blocks and signals that come out.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Instant;

use serde_json::{json, Value};

use crate::chat::formatting::{
    args_detail, args_summary, context_label, error_summary, turn_stats,
};
use crate::chat::transcript::Transcript;
use crate::pi::rpc::{AgentRecord, Launch, PiAgent};

/// Pi's thinking levels, low to high. The extended two only appear when a
/// model's `thinkingLevelMap` opts into them.
const THINKING_ORDER: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];
const THINKING_STANDARD: [&str; 5] = ["off", "minimal", "low", "medium", "high"];

/// Why pi compacted, in the terms the transcript reads in.
///
/// `overflow` is the recovery path — the request was already too big and is
/// retried after the summary — which is worth distinguishing from the threshold
/// pi aims for.
fn compaction_reason(reason: &str) -> &'static str {
    match reason {
        "threshold" => "context nearly full",
        "overflow" => "context overflowed",
        "manual" => "requested",
        _ => "",
    }
}

/// The thinking levels a model exposes, per its `thinkingLevelMap`.
///
/// A mapped string means supported, `null` means hidden, and an omitted key
/// means the standard levels through `high` are supported while `xhigh` and
/// `max` are not. Empty when the model has no reasoning at all.
pub fn available_levels(model: &Value) -> Vec<String> {
    if !model
        .get("reasoning")
        .map(|value| value.as_bool().unwrap_or(!value.is_null()))
        .unwrap_or(false)
    {
        return Vec::new();
    }
    let map = model.get("thinkingLevelMap");
    THINKING_ORDER
        .iter()
        .filter(|level| match map.and_then(|m| m.get(**level)) {
            Some(value) => !value.is_null(),
            None => THINKING_STANDARD.contains(level),
        })
        .map(|level| (*level).to_string())
        .collect()
}

/// Short session title for a message that carries only file attachments: the
/// first file's name, plus a count of the rest.
pub fn files_title(files: &[String]) -> String {
    let Some(first) = files.first() else {
        return String::new();
    };
    let name = Path::new(first)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| first.clone());
    match files.len() - 1 {
        0 => name,
        1 => format!("{name} (+1 file)"),
        extra => format!("{name} (+{extra} files)"),
    }
}

/// Everything the controller announces. The UI turns these into its own
/// notifications; nothing here knows what a signal looks like on screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signal {
    /// The agent started (`true`) or finished (`false`) a turn.
    StreamingChanged(bool),
    /// The session's on-disk file path, once discovered.
    PathKnown(String),
    /// The footer label for the current model, whenever it resolves.
    ModelKnown(String),
    /// The thinking level, whenever it resolves; empty when the model has none.
    ThinkingKnown(String),
    /// The title, once a loaded session's messages arrive.
    TitleKnown(String),
    /// Pi started (`true`) or finished (`false`) compacting.
    CompactingChanged(bool),
    /// The context readout, whenever it changes; empty when unknown.
    ContextKnown(String),
    /// The transcript changed and wants a repaint. `streaming` is set when
    /// every pending change came from the stream, so the UI may hold it back to
    /// its flush timer rather than painting now.
    TranscriptChanged { streaming: bool },
}

/// A command the controller wants sent to the agent, and what to do with the
/// reply. The owner sends it and routes the response back through
/// [`SessionController::on_response`] — which keeps this type free of the
/// process, and so testable.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingRequest {
    pub command: Value,
    pub kind: RequestKind,
}

/// What a reply is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestKind {
    /// `get_state`: the session path, the model, the thinking level.
    State,
    /// `get_session_stats`: the context-window readout.
    Stats,
    /// `get_available_models`: the model picker's list.
    Models,
    /// `set_model`, `set_thinking_level`, `prompt` — a reply that only matters
    /// when it says the command failed.
    Ack(&'static str),
}

/// A turn's accounting, for the footer under a finished reply.
#[derive(Debug, Default)]
struct Turn {
    /// When the turn began, so the busy row's elapsed time is anchored to the
    /// turn itself rather than to when the row happened to appear on screen.
    started: Option<Instant>,
    /// Wall time inside assistant messages only, so the rate reflects the model
    /// rather than the tools it waited on.
    generating: f64,
    message_started: Option<Instant>,
    /// Summed from the assistant messages this run actually streamed, rather
    /// than from `agent_end`'s payload — which may replay earlier turns and
    /// would double-count them.
    tokens: i64,
    /// An error was already surfaced during this turn; keeps `agent_end` from
    /// reprinting the failure that streamed in as a `message_update`.
    had_error: bool,
}

pub struct SessionController {
    pub agent: PiAgent,
    pub transcript: Transcript,

    session_path: Option<String>,
    model_name: Option<String>,
    model_provider: Option<String>,
    model_id: Option<String>,
    last_model_label: Option<String>,

    /// `None` until the agent reports the model's supported levels; an empty
    /// list means the model has none.
    thinking_levels: Option<Vec<String>>,
    thinking_level: Option<String>,
    last_thinking_label: Option<String>,

    /// The last compaction state announced, so the busy row is only repainted
    /// on a real transition.
    last_compacting: bool,
    compacting_since: Option<Instant>,
    context: Option<String>,

    /// The first user message, which is the session's title.
    first_prompt: Option<String>,

    streaming: bool,
    streaming_since: Option<Instant>,
    turn: Turn,

    /// toolCallId -> the transcript block its result belongs to.
    tool_blocks: HashMap<String, usize>,
    /// Identities of errored messages already reported, so a replayed
    /// `agent_end` payload cannot re-announce an earlier turn's failure.
    reported_errors: HashSet<String>,

    /// Drained by the owner after every call.
    signals: Vec<Signal>,
    requests: Vec<PendingRequest>,
    /// Request id -> what its reply is for.
    pending: HashMap<String, RequestKind>,
}

impl SessionController {
    pub fn new(launch: Launch) -> Self {
        let session_path = launch.session_path.clone();
        Self {
            agent: PiAgent::new(launch),
            transcript: Transcript::new(),
            session_path,
            model_name: None,
            model_provider: None,
            model_id: None,
            last_model_label: None,
            thinking_levels: None,
            thinking_level: None,
            last_thinking_label: None,
            last_compacting: false,
            compacting_since: None,
            context: None,
            first_prompt: None,
            streaming: false,
            streaming_since: None,
            turn: Turn::default(),
            tool_blocks: HashMap::new(),
            reported_errors: HashSet::new(),
            signals: Vec::new(),
            requests: Vec::new(),
            pending: HashMap::new(),
        }
    }

    // ---- State ----------------------------------------------------------

    pub fn session_path(&self) -> Option<&str> {
        self.session_path.as_deref()
    }

    pub fn is_streaming(&self) -> bool {
        self.streaming
    }

    pub fn is_compacting(&self) -> bool {
        self.last_compacting
    }

    /// Seconds since the current turn began, for the busy row's clock.
    pub fn streaming_for(&self) -> Option<f64> {
        self.streaming_since.map(|at| at.elapsed().as_secs_f64())
    }

    pub fn compacting_for(&self) -> Option<f64> {
        self.compacting_since.map(|at| at.elapsed().as_secs_f64())
    }

    /// The session's title: the first thing the user said in it.
    pub fn title(&self) -> &str {
        self.first_prompt.as_deref().unwrap_or_default()
    }

    /// Footer text for the current model: the friendly name if known, else the
    /// raw id (still concrete), else `None` while nothing is known yet.
    pub fn model_label(&self) -> Option<&str> {
        self.model_name.as_deref().or(self.model_id.as_deref())
    }

    /// Footer text for the current effort, or `None` when the model has no
    /// thinking levels — a *confirmed* empty list — so the selector can hide.
    pub fn thinking_label(&self) -> Option<&str> {
        match self.thinking_levels.as_deref() {
            Some([]) => None,
            _ => self.thinking_level.as_deref(),
        }
    }

    pub fn thinking_levels(&self) -> &[String] {
        self.thinking_levels.as_deref().unwrap_or_default()
    }

    pub fn context_label(&self) -> Option<&str> {
        self.context.as_deref()
    }

    /// Take everything the controller wants to announce.
    pub fn take_signals(&mut self) -> Vec<Signal> {
        std::mem::take(&mut self.signals)
    }

    /// Take everything the controller wants sent to the agent. The owner sends
    /// each one and hands the reply back to [`on_response`](Self::on_response)
    /// under the id the send returned.
    pub fn take_requests(&mut self) -> Vec<PendingRequest> {
        std::mem::take(&mut self.requests)
    }

    /// Record which pending request an id belongs to, once the owner has sent
    /// it and learned the id.
    pub fn register(&mut self, id: &str, kind: RequestKind) {
        self.pending.insert(id.to_string(), kind);
    }

    fn emit(&mut self, signal: Signal) {
        self.signals.push(signal);
    }

    fn ask(&mut self, command: Value, kind: RequestKind) {
        self.requests.push(PendingRequest { command, kind });
    }

    /// Announce a transcript repaint if one is outstanding.
    fn flush_transcript(&mut self) {
        let streaming = self.transcript.is_streaming();
        if self.transcript.take_dirty() {
            self.emit(Signal::TranscriptChanged { streaming });
        }
    }

    // ---- Chat -> agent ---------------------------------------------------

    /// Send a prompt, with any images and file paths attached to it.
    pub fn prompt(&mut self, text: &str, images: &[Value], files: &[String]) -> String {
        // Wire message: pi's prompt carries images structurally but has no file
        // channel, so file paths ride along in the text. Providers reject an
        // empty message, so an image-only turn still needs a carrier line.
        let mut message = text.to_string();
        if !files.is_empty() {
            let refs: Vec<String> = files.iter().map(|path| format!("- {path}")).collect();
            let separator = if message.is_empty() { "" } else { "\n\n" };
            message = format!("{message}{separator}Attached files:\n{}", refs.join("\n"));
        }
        if message.is_empty() && !images.is_empty() {
            message = "(see attached image)".to_string();
        }

        // The title and the transcript stay the user's intent, never the path
        // blob the wire message became.
        let display = if !text.is_empty() {
            text.to_string()
        } else if !files.is_empty() {
            files_title(files)
        } else {
            "(image)".to_string()
        };
        if self.first_prompt.is_none() {
            self.first_prompt = Some(display.clone());
            let title = display.clone();
            self.emit(Signal::TitleKnown(title));
        }
        self.transcript.add_user(&display, Vec::new());
        for (noun, count) in [("image", images.len()), ("file", files.len())] {
            if count > 0 {
                let plural = if count == 1 {
                    noun.to_string()
                } else {
                    format!("{noun}s")
                };
                self.transcript
                    .add_info(&format!("({count} {plural} attached)"), false);
            }
        }
        if self.streaming {
            self.transcript
                .add_info("(queued: delivered after the current turn)", false);
        } else if self.last_compacting {
            self.transcript
                .add_info("(queued: delivered after the compaction)", false);
        }
        self.flush_transcript();
        self.sync_state();
        message
    }

    /// Ask the agent for the session path, model and thinking level.
    pub fn sync_state(&mut self) {
        self.ask(json!({ "type": "get_state" }), RequestKind::State);
    }

    /// Ask for the model picker's list. The current selection is refreshed
    /// first; the agent answers commands in order, so it lands before the list.
    pub fn request_models(&mut self) {
        self.sync_state();
        self.ask(
            json!({ "type": "get_available_models" }),
            RequestKind::Models,
        );
    }

    pub fn set_model(&mut self, provider: &str, model_id: &str) {
        self.ask(
            json!({ "type": "set_model", "provider": provider, "modelId": model_id }),
            RequestKind::Ack("Model switch failed"),
        );
        // Optimistic, and corrected by the get_state that follows: the picker
        // should show what was chosen rather than what was there before.
        self.model_provider = Some(provider.to_string());
        self.model_id = Some(model_id.to_string());
        self.model_name = None;
        self.announce_model();
        self.sync_state();
    }

    pub fn set_thinking_level(&mut self, level: &str) {
        self.ask(
            json!({ "type": "set_thinking_level", "level": level }),
            RequestKind::Ack("Effort switch failed"),
        );
        self.thinking_level = Some(level.to_string());
        self.announce_thinking();
    }

    /// Re-read the context-window usage behind the busy row's readout.
    ///
    /// Guarded on a live agent: every RPC command starts pi if it is not
    /// running, so an unguarded poll would spawn a process for a session the
    /// reader only glanced at. Nothing is lost by skipping — with no agent
    /// there is no context to report.
    fn refresh_context(&mut self) {
        if !self.agent.running() {
            return;
        }
        self.ask(
            json!({ "type": "get_session_stats" }),
            RequestKind::Stats,
        );
    }

    // ---- Stopping ---------------------------------------------------------

    /// Ask the agent to end the current turn.
    pub fn stop(&mut self) {
        let _ = self.agent.abort();
    }

    /// Give up on a turn the agent will not end by itself.
    ///
    /// `abort` is a message, so it only lands if pi is in a state to read and
    /// act on it; a turn wedged below that level — a tool call whose promise
    /// never settles — ignores it and streams forever. Killing the process is
    /// the only way out, and the session survives it: pi has already written
    /// the conversation to disk, and the next prompt respawns against the same
    /// file. What is lost is whatever the killed turn had not committed yet,
    /// which is why this is offered rather than done automatically.
    pub fn force_stop(&mut self) {
        let was_streaming = self.streaming;
        self.agent.stop();
        self.transcript.add_info(
            "Stopped Pi. The unfinished turn was discarded; the session \
             resumes on your next message.",
            true,
        );
        if was_streaming {
            self.set_streaming(false);
        }
        self.flush_transcript();
    }

    /// Retire the session: kill its agent process. The transcript stays valid
    /// but stops updating.
    pub fn shutdown(&mut self) {
        self.agent.stop();
    }

    // ---- Agent -> chat ----------------------------------------------------

    /// Take everything the agent has said and fold it in.
    pub fn pump(&mut self) {
        for record in self.agent.drain() {
            self.on_record(record);
        }
        self.flush_transcript();
    }

    pub fn on_record(&mut self, record: AgentRecord) {
        match record {
            AgentRecord::Event(event) => self.on_event(&event),
            AgentRecord::Response { id, value } => self.on_response(&id, &value),
            AgentRecord::Notify { message, level } => {
                self.transcript.add_info(&message, level == "error");
            }
            AgentRecord::Failed(error) => {
                self.transcript
                    .add_info(&format!("Pi agent unavailable: {error}"), true);
                self.set_streaming(false);
            }
            AgentRecord::Finished { died_mid_turn } => {
                // A clean exit skips `Failed`, so this is the only place a
                // mid-turn death is noticed. Without it the session would sit
                // on "Pi is working…" forever and Stop would point at a dead
                // process.
                if died_mid_turn {
                    self.transcript
                        .add_info("Pi agent exited before finishing the turn.", true);
                    self.set_streaming(false);
                }
            }
        }
        self.flush_transcript();
    }

    /// Route one command's reply.
    pub fn on_response(&mut self, id: &str, response: &Value) {
        let Some(kind) = self.pending.remove(id) else {
            return; // nobody is waiting for it any more
        };
        let data = response.get("data").cloned().unwrap_or(Value::Null);
        match kind {
            RequestKind::State => self.absorb_state(&data),
            RequestKind::Stats => {
                let label = context_label(data.get("contextUsage"));
                if label.as_deref() != self.context.as_deref() {
                    self.context = label.clone();
                    self.emit(Signal::ContextKnown(label.unwrap_or_default()));
                }
            }
            RequestKind::Models => {}
            RequestKind::Ack(what) => {
                if response.get("success").and_then(Value::as_bool) != Some(true) {
                    let error = response
                        .get("error")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown error");
                    self.transcript
                        .add_info(&format!("{what}: {error}"), true);
                }
            }
        }
        self.flush_transcript();
    }

    /// The models a `get_available_models` reply carried, for the picker.
    pub fn models_from(response: &Value) -> Vec<Value> {
        response
            .get("data")
            .and_then(|data| data.get("models"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// Fold one `get_state` payload into the cached model, path and thinking.
    fn absorb_state(&mut self, data: &Value) {
        if let Some(file) = data.get("sessionFile").and_then(Value::as_str) {
            if !file.is_empty() && Some(file) != self.session_path.as_deref() {
                self.session_path = Some(file.to_string());
                // The agent needs it too: it launches with `--session` and
                // would otherwise respawn into an empty session after a
                // mid-turn death.
                self.agent.set_session_path(file);
                self.emit(Signal::PathKnown(file.to_string()));
            }
        }
        // A session can be mid-compaction when we attach to it — a reconnect,
        // or a prompt sent while the previous turn was still summarising — and
        // no compaction_start is replayed for one already under way.
        if let Some(compacting) = data.get("isCompacting").and_then(Value::as_bool) {
            self.agent.set_compacting(compacting);
            self.announce_compacting(compacting);
        }
        let model = data.get("model").cloned().unwrap_or(Value::Null);
        self.thinking_levels = Some(available_levels(&model));
        if let Some(level) = data.get("thinkingLevel").and_then(Value::as_str) {
            if !level.is_empty() {
                self.thinking_level = Some(level.to_string());
            }
        }
        self.set_current_model(&model);
        self.announce_thinking();
    }

    fn set_current_model(&mut self, model: &Value) {
        if let Some(provider) = model.get("provider").and_then(Value::as_str) {
            self.model_provider = Some(provider.to_string());
        }
        if let Some(id) = model.get("id").and_then(Value::as_str) {
            self.model_id = Some(id.to_string());
        }
        if let Some(name) = model.get("name").and_then(Value::as_str) {
            self.model_name = Some(name.to_string());
        }
        self.announce_model();
    }

    /// Announce whenever the effective label changes, so a session that only
    /// knows its id — the name not yet resolved — still shows a concrete model
    /// instead of staying on the placeholder.
    fn announce_model(&mut self) {
        let label = self.model_label().map(str::to_string);
        if let Some(label) = label {
            if Some(&label) != self.last_model_label.as_ref() {
                self.last_model_label = Some(label.clone());
                self.emit(Signal::ModelKnown(label));
            }
        }
    }

    fn announce_thinking(&mut self) {
        let label = self.thinking_label().unwrap_or_default().to_string();
        if Some(&label) != self.last_thinking_label.as_ref() {
            self.last_thinking_label = Some(label.clone());
            self.emit(Signal::ThinkingKnown(label));
        }
    }

    /// Announce the compaction state, on transitions only — `get_state` can
    /// report a compaction the events already told us about.
    fn announce_compacting(&mut self, compacting: bool) {
        if compacting == self.last_compacting {
            return;
        }
        self.last_compacting = compacting;
        self.compacting_since = compacting.then(Instant::now);
        self.emit(Signal::CompactingChanged(compacting));
    }

    fn set_streaming(&mut self, streaming: bool) {
        if streaming == self.streaming {
            return;
        }
        self.streaming = streaming;
        if !streaming {
            self.streaming_since = None;
        }
        self.emit(Signal::StreamingChanged(streaming));
    }

    /// True the first time an errored message is seen, false on repeats, so a
    /// replayed `agent_end` payload does not re-announce old failures.
    fn mark_error_reported(&mut self, message: &Value) -> bool {
        let key = ["responseId", "timestamp", "errorMessage"]
            .iter()
            .find_map(|field| message.get(*field).filter(|v| !v.is_null()))
            .map(|value| value.to_string())
            .unwrap_or_default();
        self.reported_errors.insert(key)
    }

    fn on_event(&mut self, event: &Value) {
        match event.get("type").and_then(Value::as_str).unwrap_or_default() {
            "agent_start" => {
                self.turn = Turn {
                    started: Some(Instant::now()),
                    ..Turn::default()
                };
                self.streaming_since = self.turn.started;
                self.set_streaming(true);
                self.refresh_context();
            }
            "message_start" => {
                if role_of(event) == Some("assistant") {
                    self.turn.message_started = Some(Instant::now());
                }
            }
            "message_end" => {
                let message = event.get("message").cloned().unwrap_or(Value::Null);
                self.absorb_finished_message(&message);
                // Only assistant messages move the reading: pi measures the
                // context from the usage a response reports, so polling after a
                // user or tool message would spend a round trip on the same
                // number.
                if message.get("role").and_then(Value::as_str) == Some("assistant") {
                    self.refresh_context();
                }
            }
            "agent_end" => {
                // A request the provider rejected outright ends the run with an
                // errored assistant message but streams no message_update
                // events — without this the turn fails silently.
                if !self.turn.had_error {
                    let messages = event
                        .get("messages")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    for message in messages {
                        if let Some(error) = error_summary(&message) {
                            if self.mark_error_reported(&message) {
                                self.transcript
                                    .add_info(&format!("Turn failed: {error}"), true);
                            }
                        }
                    }
                }
                // A run that will be retried is not the end of the reply, so it
                // gets no footer; the retry's own agent_end carries the whole
                // turn, since only agent_start resets the counters.
                if event.get("willRetry").and_then(Value::as_bool) != Some(true) {
                    self.add_turn_footer();
                }
                self.set_streaming(false);
                self.refresh_context();
            }
            "message_update" => self.on_delta(event),
            "compaction_start" => {
                self.announce_compacting(true);
            }
            "compaction_end" => {
                self.announce_compacting(false);
                self.report_compaction(event);
                self.refresh_context();
            }
            "tool_execution_start" => {
                let name = event
                    .get("toolName")
                    .and_then(Value::as_str)
                    .unwrap_or("?")
                    .to_string();
                let args = event.get("args").cloned().unwrap_or(Value::Null);
                let index = self.transcript.add_tool(
                    &name,
                    &args_summary(&args),
                    &args_detail(&args),
                );
                if let Some(id) = event.get("toolCallId").and_then(Value::as_str) {
                    self.tool_blocks.insert(id.to_string(), index);
                }
            }
            "tool_execution_end" => {
                let index = event
                    .get("toolCallId")
                    .and_then(Value::as_str)
                    .and_then(|id| self.tool_blocks.remove(id));
                let text = tool_result_text(event.get("result"));
                if let (Some(index), false) = (index, text.is_empty()) {
                    let text = if event.get("isError").and_then(Value::as_bool) == Some(true) {
                        format!("(error)\n{text}")
                    } else {
                        text
                    };
                    self.transcript.append_tool_detail(index, &text);
                }
            }
            _ => {}
        }
    }

    fn on_delta(&mut self, event: &Value) {
        let delta = event
            .get("assistantMessageEvent")
            .cloned()
            .unwrap_or(Value::Null);
        let text = delta.get("delta").and_then(Value::as_str).unwrap_or_default();
        match delta.get("type").and_then(Value::as_str).unwrap_or_default() {
            "text_delta" => self.transcript.append_assistant_delta(text),
            "thinking_delta" => self.transcript.append_thinking_delta(text),
            "error" => {
                self.turn.had_error = true;
                let reason = delta
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("error");
                // An aborted turn is the user's own doing, so it is a note
                // rather than a failure.
                self.transcript
                    .add_info(&format!("({reason})"), reason != "aborted");
            }
            _ => {}
        }
    }

    /// Fold one completed assistant message into the turn's totals.
    ///
    /// Pi emits `message_end` for user and tool messages too, and those neither
    /// generate tokens nor take time worth attributing to the model.
    fn absorb_finished_message(&mut self, message: &Value) {
        if message.get("role").and_then(Value::as_str) != Some("assistant") {
            return;
        }
        if let Some(started) = self.turn.message_started.take() {
            self.turn.generating += started.elapsed().as_secs_f64();
        }
        // A provider that reports something unexpected costs a footer's
        // accuracy, not a turn.
        if let Some(output) = message
            .get("usage")
            .and_then(|usage| usage.get("output"))
            .and_then(Value::as_i64)
        {
            self.turn.tokens += output;
        }
    }

    /// Close the reply with what it cost.
    ///
    /// Skipped when the turn never started a clock — a run that failed before
    /// reaching the model — since a bare `0.0s` says less than nothing.
    fn add_turn_footer(&mut self) {
        let Some(started) = self.turn.started else {
            return;
        };
        let stats = turn_stats(
            started.elapsed().as_secs_f64(),
            self.turn.generating,
            self.turn.tokens,
        );
        self.transcript.add_footer(&stats);
    }

    /// One transcript line for a finished compaction.
    ///
    /// Compaction rewrites the history the next turn is built on, so it is a
    /// fact about the conversation rather than a status: the busy row's
    /// "compacting" state is gone the moment it ends, and without a line here a
    /// session that silently lost its older messages looks the same as one that
    /// never had them.
    fn report_compaction(&mut self, event: &Value) {
        let reason = compaction_reason(
            event.get("reason").and_then(Value::as_str).unwrap_or_default(),
        );
        let suffix = if reason.is_empty() {
            String::new()
        } else {
            format!(" ({reason})")
        };
        let Some(result) = event.get("result").filter(|r| r.is_object()) else {
            if event.get("aborted").and_then(Value::as_bool) == Some(true) {
                self.transcript
                    .add_info(&format!("Compaction cancelled{suffix}"), false);
            } else {
                let error = event
                    .get("errorMessage")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown error");
                self.transcript
                    .add_info(&format!("Compaction failed{suffix}: {error}"), true);
            }
            return;
        };
        let before = result.get("tokensBefore").and_then(Value::as_f64);
        let after = result.get("estimatedTokensAfter").and_then(Value::as_f64);
        let mut text = format!("Context compacted{suffix}");
        if let (Some(before), Some(after)) = (before, after) {
            text.push_str(&format!(
                ": {} → {} tok",
                thousands(before as i64),
                thousands(after as i64)
            ));
        }
        self.transcript.add_info(&text, false);
    }
}

fn role_of(event: &Value) -> Option<&str> {
    event
        .get("message")
        .and_then(|message| message.get("role"))
        .and_then(Value::as_str)
}

/// The text parts of a tool result, joined and trimmed.
fn tool_result_text(result: Option<&Value>) -> String {
    let Some(parts) = result
        .and_then(|result| result.get("content"))
        .and_then(Value::as_array)
    else {
        return String::new();
    };
    parts
        .iter()
        .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// A count with thousands separators, as the compaction line reports them.
fn thousands(value: i64) -> String {
    let negative = value < 0;
    let digits = value.abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    if negative {
        format!("-{out}")
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::transcript::Block;

    /// A controller with no child process behind it: every test here drives it
    /// with the records pi would have sent.
    fn controller() -> SessionController {
        let mut controller = SessionController::new(Launch::new("/tmp"));
        controller.agent = PiAgent::offline(Launch::new("/tmp"));
        controller
    }

    fn event(controller: &mut SessionController, value: Value) {
        controller.on_record(AgentRecord::Event(value));
    }

    fn kinds(controller: &SessionController) -> Vec<&'static str> {
        controller.transcript.blocks().iter().map(Block::kind).collect()
    }

    fn signals(controller: &mut SessionController) -> Vec<Signal> {
        controller.take_signals()
    }

    #[test]
    fn a_model_without_reasoning_offers_no_levels() {
        assert!(available_levels(&json!({ "id": "gpt-4" })).is_empty());
        assert!(available_levels(&json!({ "reasoning": false })).is_empty());
    }

    #[test]
    fn a_reasoning_model_offers_the_standard_levels_by_default() {
        assert_eq!(
            available_levels(&json!({ "reasoning": true })),
            ["off", "minimal", "low", "medium", "high"]
        );
    }

    #[test]
    fn a_thinking_map_opts_into_the_extended_levels_and_hides_others() {
        let model = json!({
            "reasoning": true,
            "thinkingLevelMap": {
                "off": "none",
                "minimal": null,
                "low": "low",
                "medium": "medium",
                "high": "high",
                "xhigh": "xhigh"
            }
        });
        // `minimal` is mapped to null, so it is hidden; `xhigh` was opted into;
        // `max` was not mentioned and is not standard, so it stays out.
        assert_eq!(
            available_levels(&model),
            ["off", "low", "medium", "high", "xhigh"]
        );
    }

    #[test]
    fn a_file_only_prompt_is_titled_by_its_first_file() {
        assert_eq!(files_title(&["/a/b/notes.md".into()]), "notes.md");
        assert_eq!(
            files_title(&["/a/notes.md".into(), "/a/x.rs".into()]),
            "notes.md (+1 file)"
        );
        assert_eq!(
            files_title(&["/a/notes.md".into(), "/a/x.rs".into(), "/a/y.rs".into()]),
            "notes.md (+2 files)"
        );
        assert_eq!(files_title(&[]), "");
    }

    #[test]
    fn a_prompt_shows_what_the_user_typed_and_sends_the_paths_along() {
        let mut controller = controller();
        let wire = controller.prompt("fix this", &[], &["/a/x.rs".into()]);
        // The wire message carries the paths; the transcript shows the intent.
        assert!(wire.starts_with("fix this\n\nAttached files:\n- /a/x.rs"));
        match &controller.transcript.blocks()[0] {
            Block::User { text, .. } => assert_eq!(text, "fix this"),
            other => panic!("expected a user block, got {other:?}"),
        }
        assert_eq!(kinds(&controller), ["user", "info"]);
    }

    #[test]
    fn an_image_only_prompt_still_carries_a_line_for_the_provider() {
        let mut controller = controller();
        let wire = controller.prompt("", &[json!({ "type": "image" })], &[]);
        // Providers reject an empty message.
        assert_eq!(wire, "(see attached image)");
        match &controller.transcript.blocks()[0] {
            Block::User { text, .. } => assert_eq!(text, "(image)"),
            other => panic!("expected a user block, got {other:?}"),
        }
    }

    #[test]
    fn the_first_prompt_becomes_the_sessions_title() {
        let mut controller = controller();
        controller.prompt("add a parser", &[], &[]);
        assert_eq!(controller.title(), "add a parser");
        assert!(signals(&mut controller)
            .contains(&Signal::TitleKnown("add a parser".into())));
        // A later prompt does not rename the session.
        controller.prompt("and a test", &[], &[]);
        assert_eq!(controller.title(), "add a parser");
    }

    #[test]
    fn a_prompt_sent_mid_turn_says_it_is_queued() {
        let mut controller = controller();
        event(&mut controller, json!({ "type": "agent_start" }));
        controller.prompt("also this", &[], &[]);
        let text: Vec<String> = controller
            .transcript
            .blocks()
            .iter()
            .filter_map(|block| match block {
                Block::Info { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert!(text.iter().any(|t| t.contains("after the current turn")), "{text:?}");
    }

    #[test]
    fn a_turn_streams_into_the_transcript_and_closes_with_its_stats() {
        let mut controller = controller();
        event(&mut controller, json!({ "type": "agent_start" }));
        assert!(controller.is_streaming());
        event(
            &mut controller,
            json!({
                "type": "message_start",
                "message": { "role": "assistant" }
            }),
        );
        for delta in ["Here ", "you ", "go."] {
            event(
                &mut controller,
                json!({
                    "type": "message_update",
                    "assistantMessageEvent": { "type": "text_delta", "delta": delta }
                }),
            );
        }
        event(
            &mut controller,
            json!({
                "type": "message_end",
                "message": { "role": "assistant", "usage": { "output": 128 } }
            }),
        );
        event(&mut controller, json!({ "type": "agent_end" }));

        assert!(!controller.is_streaming());
        assert_eq!(kinds(&controller), ["assistant", "footer"]);
        match &controller.transcript.blocks()[0] {
            Block::Assistant { markdown } => assert_eq!(markdown, "Here you go."),
            other => panic!("expected an assistant block, got {other:?}"),
        }
        match &controller.transcript.blocks()[1] {
            Block::Footer { text } => assert!(text.contains("128"), "{text:?}"),
            other => panic!("expected a footer, got {other:?}"),
        }
    }

    #[test]
    fn reasoning_and_reply_land_in_blocks_of_their_own() {
        let mut controller = controller();
        event(
            &mut controller,
            json!({
                "type": "message_update",
                "assistantMessageEvent": { "type": "thinking_delta", "delta": "Hmm." }
            }),
        );
        event(
            &mut controller,
            json!({
                "type": "message_update",
                "assistantMessageEvent": { "type": "text_delta", "delta": "Right." }
            }),
        );
        assert_eq!(kinds(&controller), ["thinking", "assistant"]);
    }

    #[test]
    fn a_tool_call_and_its_result_are_one_block() {
        let mut controller = controller();
        event(
            &mut controller,
            json!({
                "type": "tool_execution_start",
                "toolCallId": "t1",
                "toolName": "read",
                "args": { "path": "src/main.rs" }
            }),
        );
        event(
            &mut controller,
            json!({
                "type": "tool_execution_end",
                "toolCallId": "t1",
                "result": { "content": [{ "type": "text", "text": "fn main() {}" }] }
            }),
        );
        assert_eq!(kinds(&controller), ["tool"]);
        match &controller.transcript.blocks()[0] {
            Block::Tool { name, detail, .. } => {
                assert_eq!(name, "read");
                assert!(detail.ends_with("fn main() {}"), "{detail:?}");
            }
            other => panic!("expected a tool block, got {other:?}"),
        }
    }

    #[test]
    fn a_failed_tool_says_so_in_its_detail() {
        let mut controller = controller();
        event(
            &mut controller,
            json!({
                "type": "tool_execution_start", "toolCallId": "t1", "toolName": "bash"
            }),
        );
        event(
            &mut controller,
            json!({
                "type": "tool_execution_end",
                "toolCallId": "t1",
                "isError": true,
                "result": { "content": [{ "type": "text", "text": "no such file" }] }
            }),
        );
        match &controller.transcript.blocks()[0] {
            Block::Tool { detail, .. } => assert!(detail.contains("(error)"), "{detail:?}"),
            other => panic!("expected a tool block, got {other:?}"),
        }
    }

    #[test]
    fn a_provider_rejection_with_no_stream_is_still_reported() {
        let mut controller = controller();
        event(&mut controller, json!({ "type": "agent_start" }));
        event(
            &mut controller,
            json!({
                "type": "agent_end",
                "messages": [{
                    "role": "assistant",
                    "stopReason": "error",
                    "responseId": "r1",
                    "errorMessage": "context length exceeded"
                }]
            }),
        );
        let reported: Vec<String> = controller
            .transcript
            .blocks()
            .iter()
            .filter_map(|b| match b {
                Block::Info { text, error: true } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(reported.len(), 1);
        assert!(reported[0].contains("context length exceeded"), "{reported:?}");
    }

    #[test]
    fn a_replayed_agent_end_does_not_re_announce_an_old_failure() {
        let mut controller = controller();
        let failed = json!({
            "role": "assistant",
            "stopReason": "error",
            "responseId": "r1",
            "errorMessage": "boom"
        });
        for _ in 0..3 {
            event(&mut controller, json!({ "type": "agent_start" }));
            event(
                &mut controller,
                json!({ "type": "agent_end", "messages": [failed] }),
            );
        }
        let reported = controller
            .transcript
            .blocks()
            .iter()
            .filter(|b| matches!(b, Block::Info { error: true, .. }))
            .count();
        assert_eq!(reported, 1, "the same failure was announced more than once");
    }

    #[test]
    fn an_error_that_already_streamed_is_not_reprinted_at_the_end() {
        let mut controller = controller();
        event(&mut controller, json!({ "type": "agent_start" }));
        event(
            &mut controller,
            json!({
                "type": "message_update",
                "assistantMessageEvent": { "type": "error", "reason": "overloaded" }
            }),
        );
        event(
            &mut controller,
            json!({
                "type": "agent_end",
                "messages": [{
                    "role": "assistant", "stopReason": "error", "errorMessage": "overloaded"
                }]
            }),
        );
        let reported = controller
            .transcript
            .blocks()
            .iter()
            .filter(|b| matches!(b, Block::Info { error: true, .. }))
            .count();
        assert_eq!(reported, 1);
    }

    #[test]
    fn an_aborted_turn_is_a_note_rather_than_a_failure() {
        let mut controller = controller();
        event(
            &mut controller,
            json!({
                "type": "message_update",
                "assistantMessageEvent": { "type": "error", "reason": "aborted" }
            }),
        );
        match &controller.transcript.blocks()[0] {
            Block::Info { text, error } => {
                assert_eq!(text, "(aborted)");
                assert!(!error, "the user's own stop is not an error");
            }
            other => panic!("expected an info block, got {other:?}"),
        }
    }

    #[test]
    fn a_retried_run_gets_no_footer_of_its_own() {
        let mut controller = controller();
        event(&mut controller, json!({ "type": "agent_start" }));
        event(&mut controller, json!({ "type": "agent_end", "willRetry": true }));
        assert!(kinds(&controller).is_empty());
        // The retry's own end carries the whole turn.
        event(&mut controller, json!({ "type": "agent_end" }));
        assert_eq!(kinds(&controller), ["footer"]);
    }

    #[test]
    fn a_turn_that_never_reached_the_model_gets_no_footer() {
        let mut controller = controller();
        // No agent_start, so no clock: a bare "0.0s" says less than nothing.
        event(&mut controller, json!({ "type": "agent_end" }));
        assert!(kinds(&controller).is_empty());
    }

    #[test]
    fn streaming_is_announced_on_transitions_only() {
        let mut controller = controller();
        event(&mut controller, json!({ "type": "agent_start" }));
        event(&mut controller, json!({ "type": "agent_start" }));
        event(&mut controller, json!({ "type": "agent_end" }));
        let changes: Vec<&Signal> = controller
            .signals
            .iter()
            .filter(|s| matches!(s, Signal::StreamingChanged(_)))
            .collect();
        assert_eq!(
            changes,
            [&Signal::StreamingChanged(true), &Signal::StreamingChanged(false)]
        );
    }

    #[test]
    fn compaction_is_announced_once_and_leaves_a_line_behind() {
        let mut controller = controller();
        event(&mut controller, json!({ "type": "compaction_start" }));
        assert!(controller.is_compacting());
        event(
            &mut controller,
            json!({
                "type": "compaction_end",
                "reason": "threshold",
                "result": { "tokensBefore": 120000, "estimatedTokensAfter": 8000 }
            }),
        );
        assert!(!controller.is_compacting());
        match &controller.transcript.blocks()[0] {
            Block::Info { text, error } => {
                assert_eq!(
                    text,
                    "Context compacted (context nearly full): 120,000 → 8,000 tok"
                );
                assert!(!error);
            }
            other => panic!("expected an info block, got {other:?}"),
        }
    }

    #[test]
    fn a_failed_or_cancelled_compaction_says_which() {
        let mut controller = controller();
        event(
            &mut controller,
            json!({ "type": "compaction_end", "aborted": true, "reason": "manual" }),
        );
        event(
            &mut controller,
            json!({ "type": "compaction_end", "errorMessage": "out of memory" }),
        );
        let text: Vec<String> = controller
            .transcript
            .blocks()
            .iter()
            .filter_map(|b| match b {
                Block::Info { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(text[0], "Compaction cancelled (requested)");
        assert_eq!(text[1], "Compaction failed: out of memory");
    }

    #[test]
    fn thousands_separates_the_way_the_compaction_line_reads() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1000), "1,000");
        assert_eq!(thousands(120_000), "120,000");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn the_session_path_is_learned_from_the_agents_state() {
        let mut controller = controller();
        controller.sync_state();
        let requests = controller.take_requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].kind, RequestKind::State);
        controller.register("req-1", RequestKind::State);
        controller.on_response(
            "req-1",
            &json!({
                "success": true,
                "data": { "sessionFile": "/home/x/.pi/agent/sessions/abc.jsonl" }
            }),
        );
        assert_eq!(
            controller.session_path(),
            Some("/home/x/.pi/agent/sessions/abc.jsonl")
        );
        assert!(signals(&mut controller).contains(&Signal::PathKnown(
            "/home/x/.pi/agent/sessions/abc.jsonl".into()
        )));
    }

    #[test]
    fn a_model_that_only_knows_its_id_still_names_something_concrete() {
        let mut controller = controller();
        controller.register("req-1", RequestKind::State);
        controller.on_response(
            "req-1",
            &json!({ "data": { "model": { "id": "claude-opus-5", "provider": "anthropic" } } }),
        );
        assert_eq!(controller.model_label(), Some("claude-opus-5"));
        // The friendly name wins once it resolves.
        controller.register("req-2", RequestKind::State);
        controller.on_response(
            "req-2",
            &json!({ "data": { "model": { "id": "claude-opus-5", "name": "Opus 5" } } }),
        );
        assert_eq!(controller.model_label(), Some("Opus 5"));
        let announced: Vec<&Signal> = controller
            .signals
            .iter()
            .filter(|s| matches!(s, Signal::ModelKnown(_)))
            .collect();
        assert_eq!(
            announced,
            [
                &Signal::ModelKnown("claude-opus-5".into()),
                &Signal::ModelKnown("Opus 5".into())
            ]
        );
    }

    #[test]
    fn a_model_with_no_thinking_hides_the_effort_selector() {
        let mut controller = controller();
        controller.register("req-1", RequestKind::State);
        controller.on_response(
            "req-1",
            &json!({ "data": { "model": { "id": "m" }, "thinkingLevel": "high" } }),
        );
        // A confirmed-empty level list means the selector has nothing to show,
        // whatever level the session happens to carry.
        assert_eq!(controller.thinking_label(), None);
        assert!(controller.thinking_levels().is_empty());
    }

    #[test]
    fn a_reasoning_model_reports_its_level_and_choices() {
        let mut controller = controller();
        controller.register("req-1", RequestKind::State);
        controller.on_response(
            "req-1",
            &json!({
                "data": {
                    "model": { "id": "m", "reasoning": true },
                    "thinkingLevel": "medium"
                }
            }),
        );
        assert_eq!(controller.thinking_label(), Some("medium"));
        assert_eq!(controller.thinking_levels().len(), 5);
        assert!(signals(&mut controller).contains(&Signal::ThinkingKnown("medium".into())));
    }

    #[test]
    fn a_rejected_command_says_so_in_the_transcript() {
        let mut controller = controller();
        controller.register("req-1", RequestKind::Ack("Model switch failed"));
        controller.on_response(
            "req-1",
            &json!({ "success": false, "error": "no such model" }),
        );
        match &controller.transcript.blocks()[0] {
            Block::Info { text, error } => {
                assert_eq!(text, "Model switch failed: no such model");
                assert!(error);
            }
            other => panic!("expected an info block, got {other:?}"),
        }
    }

    #[test]
    fn a_successful_command_says_nothing() {
        let mut controller = controller();
        controller.register("req-1", RequestKind::Ack("Model switch failed"));
        controller.on_response("req-1", &json!({ "success": true }));
        assert!(controller.transcript.is_empty());
    }

    #[test]
    fn a_reply_nobody_is_waiting_for_is_dropped() {
        let mut controller = controller();
        controller.on_response("req-99", &json!({ "success": false, "error": "late" }));
        assert!(controller.transcript.is_empty());
    }

    #[test]
    fn the_context_readout_is_announced_only_when_it_moves() {
        let mut controller = controller();
        let usage = json!({
            "success": true,
            "data": { "contextUsage": { "tokens": 4000, "contextWindow": 200000 } }
        });
        controller.register("req-1", RequestKind::Stats);
        controller.on_response("req-1", &usage);
        let first = signals(&mut controller);
        assert!(first.iter().any(|s| matches!(s, Signal::ContextKnown(_))));
        controller.register("req-2", RequestKind::Stats);
        controller.on_response("req-2", &usage);
        assert!(!signals(&mut controller)
            .iter()
            .any(|s| matches!(s, Signal::ContextKnown(_))));
    }

    #[test]
    fn a_stop_that_never_landed_can_be_escalated_to_killing_pi() {
        let mut controller = controller();
        event(&mut controller, json!({ "type": "agent_start" }));
        assert!(controller.is_streaming());
        // `abort` is a message; a wedged turn ignores it and keeps streaming.
        controller.stop();
        assert!(controller.is_streaming());
        // Killing the process is the only way out, and it clears the busy row.
        controller.force_stop();
        assert!(!controller.is_streaming());
        match controller.transcript.blocks().last() {
            Some(Block::Info { text, error }) => {
                assert!(text.starts_with("Stopped Pi."), "{text:?}");
                assert!(error);
            }
            other => panic!("expected an info block, got {other:?}"),
        }
        assert!(signals(&mut controller).contains(&Signal::StreamingChanged(false)));
    }

    #[test]
    fn a_mid_turn_death_clears_the_busy_row_and_says_what_happened() {
        let mut controller = controller();
        event(&mut controller, json!({ "type": "agent_start" }));
        controller.on_record(AgentRecord::Finished {
            died_mid_turn: true,
        });
        assert!(!controller.is_streaming());
        match controller.transcript.blocks().last() {
            Some(Block::Info { text, error }) => {
                assert!(text.contains("exited before finishing"), "{text:?}");
                assert!(error);
            }
            other => panic!("expected an info block, got {other:?}"),
        }
    }

    #[test]
    fn a_clean_exit_between_turns_says_nothing() {
        let mut controller = controller();
        controller.on_record(AgentRecord::Finished {
            died_mid_turn: false,
        });
        assert!(controller.transcript.is_empty());
    }

    #[test]
    fn an_extension_notice_lands_in_the_transcript_at_its_own_level() {
        let mut controller = controller();
        controller.on_record(AgentRecord::Notify {
            message: "formatting the file".into(),
            level: "info".into(),
        });
        controller.on_record(AgentRecord::Notify {
            message: "the linter is unavailable".into(),
            level: "error".into(),
        });
        let flags: Vec<bool> = controller
            .transcript
            .blocks()
            .iter()
            .filter_map(|b| match b {
                Block::Info { error, .. } => Some(*error),
                _ => None,
            })
            .collect();
        assert_eq!(flags, [false, true]);
    }

    #[test]
    fn the_models_reply_is_unwrapped_for_the_picker() {
        let response = json!({ "data": { "models": [{ "id": "a" }, { "id": "b" }] } });
        assert_eq!(SessionController::models_from(&response).len(), 2);
        assert!(SessionController::models_from(&json!({})).is_empty());
    }

    #[test]
    fn asking_for_models_refreshes_the_selection_first() {
        let mut controller = controller();
        controller.request_models();
        let kinds: Vec<RequestKind> = controller
            .take_requests()
            .into_iter()
            .map(|r| r.kind)
            .collect();
        // The agent answers in order, so the state lands before the list does.
        assert_eq!(kinds, [RequestKind::State, RequestKind::Models]);
    }

    #[test]
    fn choosing_a_model_shows_the_choice_before_the_agent_confirms_it() {
        let mut controller = controller();
        controller.set_model("anthropic", "claude-opus-5");
        assert_eq!(controller.model_label(), Some("claude-opus-5"));
        let commands: Vec<Value> = controller
            .take_requests()
            .into_iter()
            .map(|r| r.command)
            .collect();
        assert_eq!(commands[0]["type"], "set_model");
        assert_eq!(commands[0]["modelId"], "claude-opus-5");
    }
}
