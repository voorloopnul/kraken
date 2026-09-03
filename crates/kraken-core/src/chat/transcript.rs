//! The transcript: an ordered list of blocks the conversation pane repaints
//! from.
//!
//! Nothing here is styled or laid out. A block carries what was said and what
//! kind of thing said it; the pane decides what that looks like, so a theme
//! switch is a repaint from these same blocks rather than a re-read of text
//! that was already coloured for the theme it is leaving.
//!
//! Streaming is the reason this is a model rather than a string. Deltas arrive
//! a token at a time, and re-rendering the trailing block is O(block) — painting
//! once per token is quadratic over a long reply. Deltas therefore extend the
//! trailing block in place and only *mark* the transcript dirty; the pane asks
//! [`Transcript::take_dirty`] on a timer and repaints at most that often.

use serde_json::Value;

use crate::chat::formatting;

/// How often a streaming transcript may be repainted, and the share of wall
/// time those repaints are allowed to take.
///
/// A paint is not free, and an expanded reasoning block re-parses its whole
/// markdown and re-highlights every code fence in it. Without the second rule a
/// long one would spend the entire frame budget repainting and stop answering
/// the mouse, so a costly paint pushes the next one out in proportion to what
/// it cost.
pub const FLUSH_INTERVAL_MS: u64 = 50;
pub const FLUSH_DUTY: u32 = 4;

/// How much of a reasoning block's text the collapsed row reads to build its
/// one-line summary. The summary is clipped to 100 characters, so this is
/// generous even for dense markup — and it keeps a collapsed row's cost flat
/// while the model streams into it, instead of re-reading the whole reasoning
/// on every repaint.
pub const SUMMARY_SCAN: usize = 600;
pub const SUMMARY_LIMIT: usize = 100;

/// An image or file the user attached to a prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    pub name: String,
    /// Where it came from on disk, or empty for something pasted or captured.
    pub path: String,
    /// A `data:` URL for an image, so the pane can show a thumbnail without
    /// reaching back to the file.
    pub preview: String,
}

/// One entry in the transcript.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    /// What the user sent, painted in a bubble.
    User {
        text: String,
        attachments: Vec<Attachment>,
    },
    /// The model's reply, as markdown to be rendered.
    Assistant { markdown: String },
    /// The model's reasoning: dimmed, and collapsed to one line until asked.
    Thinking { text: String, expanded: bool },
    /// A tool call: what was called, a one-line summary of the arguments, and
    /// the detail — arguments and result — behind a disclosure.
    Tool {
        name: String,
        summary: String,
        detail: String,
        expanded: bool,
    },
    /// A notice from the app or the agent. `error` picks the loud colour.
    Info { text: String, error: bool },
    /// The stats line that closes a turn.
    Footer { text: String },
}

impl Block {
    /// The block kind as a short tag, for a UI that switches on it.
    pub fn kind(&self) -> &'static str {
        match self {
            Block::User { .. } => "user",
            Block::Assistant { .. } => "assistant",
            Block::Thinking { .. } => "thinking",
            Block::Tool { .. } => "tool",
            Block::Info { .. } => "info",
            Block::Footer { .. } => "footer",
        }
    }
}

/// The one-line preview a collapsed reasoning block shows.
///
/// Only the head of the text is read: the summary is clipped anyway, and a
/// block still being streamed into would otherwise be re-scanned whole on every
/// repaint. Markdown markers are dropped rather than rendered, because this is
/// one line of plain text in a row, not a document.
pub fn thinking_summary(text: &str) -> String {
    let head: String = text.chars().take(SUMMARY_SCAN).collect();
    let mut summary = String::new();
    for line in head.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let cleaned = line
            .trim_start_matches(['#', '>', '-', '*', '+'])
            .trim_start_matches(|c: char| c.is_ascii_digit())
            .trim_start_matches(['.', ')'])
            .trim()
            .replace(['`', '*', '_'], "");
        if cleaned.is_empty() {
            continue;
        }
        if !summary.is_empty() {
            summary.push(' ');
        }
        summary.push_str(&cleaned);
        if summary.chars().count() >= SUMMARY_LIMIT {
            break;
        }
    }
    if summary.chars().count() > SUMMARY_LIMIT {
        let kept: String = summary.chars().take(SUMMARY_LIMIT - 1).collect();
        return format!("{}…", kept.trim_end());
    }
    summary
}

#[derive(Debug, Default, Clone)]
pub struct Transcript {
    blocks: Vec<Block>,
    /// Set by anything that changed a block, cleared when the pane repaints.
    dirty: bool,
    /// Whether any of the pending changes was something other than a streaming
    /// delta. The pane may hold a purely streamed repaint back to its timer,
    /// while a structural change (a tool call, a footer) repaints at once —
    /// those arrive at human speed, and waiting on one would look like a stall.
    structural: bool,
}

impl Transcript {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    pub fn clear(&mut self) {
        self.blocks.clear();
        self.dirty = true;
        self.structural = true;
    }

    /// Whether the pane has repainting to do, clearing the flag as it answers.
    pub fn take_dirty(&mut self) -> bool {
        let dirty = self.dirty;
        self.dirty = false;
        self.structural = false;
        dirty
    }

    /// Whether everything outstanding came from the stream, and so may wait for
    /// the flush timer rather than being painted now.
    pub fn is_streaming(&self) -> bool {
        self.dirty && !self.structural
    }

    fn mark(&mut self, streaming: bool) {
        self.dirty = true;
        self.structural |= !streaming;
    }

    pub fn add_user(&mut self, text: &str, attachments: Vec<Attachment>) {
        self.blocks.push(Block::User {
            text: text.to_string(),
            attachments,
        });
        self.mark(false);
    }

    /// Extend the trailing assistant block, or open one.
    pub fn append_assistant_delta(&mut self, delta: &str) {
        if delta.is_empty() {
            return;
        }
        match self.blocks.last_mut() {
            Some(Block::Assistant { markdown }) => markdown.push_str(delta),
            _ => self.blocks.push(Block::Assistant {
                markdown: delta.to_string(),
            }),
        }
        self.mark(true);
    }

    /// Extend the trailing reasoning block, or open one.
    pub fn append_thinking_delta(&mut self, delta: &str) {
        if delta.is_empty() {
            return;
        }
        match self.blocks.last_mut() {
            Some(Block::Thinking { text, .. }) => text.push_str(delta),
            _ => self.blocks.push(Block::Thinking {
                text: delta.to_string(),
                expanded: false,
            }),
        }
        self.mark(true);
    }

    /// Add a tool call and return its index, which
    /// [`append_tool_detail`](Self::append_tool_detail) needs to find it again
    /// once the result arrives — by then the model may have written several
    /// more blocks past it.
    pub fn add_tool(&mut self, name: &str, summary: &str, detail: &str) -> usize {
        self.blocks.push(Block::Tool {
            name: name.to_string(),
            summary: summary.to_string(),
            detail: detail.to_string(),
            expanded: false,
        });
        self.mark(false);
        self.blocks.len() - 1
    }

    /// Append to a tool call's detail. Out-of-range and non-tool indices are
    /// ignored: a result can outlive the transcript it belonged to (a session
    /// switch clears it) and losing the text is better than panicking.
    pub fn append_tool_detail(&mut self, index: usize, text: &str) {
        if text.is_empty() {
            return;
        }
        if let Some(Block::Tool { detail, .. }) = self.blocks.get_mut(index) {
            if !detail.is_empty() && !detail.ends_with('\n') {
                detail.push('\n');
            }
            detail.push_str(text);
            self.mark(false);
        }
    }

    /// Fold a block open or shut. Returns whether anything changed.
    pub fn set_expanded(&mut self, index: usize, open: bool) -> bool {
        let changed = match self.blocks.get_mut(index) {
            Some(Block::Thinking { expanded, .. }) | Some(Block::Tool { expanded, .. }) => {
                let changed = *expanded != open;
                *expanded = open;
                changed
            }
            _ => false,
        };
        if changed {
            self.mark(false);
        }
        changed
    }

    pub fn add_info(&mut self, text: &str, error: bool) {
        self.blocks.push(Block::Info {
            text: text.to_string(),
            error,
        });
        self.mark(false);
    }

    pub fn add_footer(&mut self, text: &str) {
        self.blocks.push(Block::Footer {
            text: text.to_string(),
        });
        self.mark(false);
    }

    /// Replace the transcript with a session's stored messages.
    ///
    /// `footers` maps a message's index to the stats line that closes its turn;
    /// [`formatting::replay_footers`] builds it from the same message list.
    pub fn render_messages(&mut self, messages: &[Value], footers: &[(usize, String)]) {
        self.clear();
        // toolCallId -> the block its result belongs to. Pi reports a call and
        // its result as two separate messages, sometimes far apart.
        let mut tool_blocks: Vec<(String, usize)> = Vec::new();
        for (index, message) in messages.iter().enumerate() {
            match message.get("role").and_then(Value::as_str) {
                Some("user") => {
                    let text = formatting::content_text(
                        message.get("content").unwrap_or(&Value::Null),
                    );
                    if !text.is_empty() {
                        self.add_user(&text, Vec::new());
                    }
                }
                Some("assistant") => {
                    if let Some(parts) = message.get("content").and_then(Value::as_array) {
                        for part in parts {
                            self.replay_assistant_part(part, &mut tool_blocks);
                        }
                    }
                    if let Some(error) = formatting::error_summary(message) {
                        self.add_info(&format!("Turn failed: {error}"), true);
                    }
                }
                Some("toolResult") => {
                    let id = message.get("toolCallId").and_then(Value::as_str);
                    let result = formatting::content_text(
                        message.get("content").unwrap_or(&Value::Null),
                    );
                    if let Some(id) = id {
                        if let Some((_, block)) =
                            tool_blocks.iter().find(|(known, _)| known == id)
                        {
                            let block = *block;
                            self.append_tool_detail(block, &result);
                        }
                    }
                }
                Some("bashExecution") => {
                    let command = message
                        .get("command")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    let output = message
                        .get("output")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    self.add_tool("bash", command, output);
                }
                _ => {}
            }
            if let Some((_, footer)) = footers.iter().find(|(at, _)| *at == index) {
                self.add_footer(footer);
            }
        }
        // A replay is one repaint, however many blocks it built.
        self.dirty = true;
        self.structural = true;
    }

    fn replay_assistant_part(&mut self, part: &Value, tools: &mut Vec<(String, usize)>) {
        match part.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    self.append_assistant_delta(text);
                }
            }
            Some("thinking") => {
                if let Some(text) = part.get("thinking").and_then(Value::as_str) {
                    self.append_thinking_delta(text);
                }
            }
            Some("toolCall") => {
                let name = part.get("name").and_then(Value::as_str).unwrap_or("?");
                let args = part.get("arguments").unwrap_or(&Value::Null);
                let index = self.add_tool(
                    name,
                    &formatting::args_summary(args),
                    &formatting::args_detail(args),
                );
                if let Some(id) = part.get("id").and_then(Value::as_str) {
                    tools.push((id.to_string(), index));
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_empty_transcript_has_nothing_to_paint() {
        let mut transcript = Transcript::new();
        assert!(transcript.is_empty());
        assert!(!transcript.take_dirty());
    }

    #[test]
    fn assistant_deltas_extend_one_block_rather_than_stacking() {
        let mut transcript = Transcript::new();
        for delta in ["Here ", "is ", "the plan."] {
            transcript.append_assistant_delta(delta);
        }
        assert_eq!(transcript.len(), 1);
        assert_eq!(
            transcript.blocks()[0],
            Block::Assistant {
                markdown: "Here is the plan.".into()
            }
        );
    }

    #[test]
    fn reasoning_and_reply_are_separate_blocks() {
        let mut transcript = Transcript::new();
        transcript.append_thinking_delta("Working it out. ");
        transcript.append_assistant_delta("Done.");
        transcript.append_thinking_delta("More.");
        assert_eq!(
            transcript
                .blocks()
                .iter()
                .map(Block::kind)
                .collect::<Vec<_>>(),
            ["thinking", "assistant", "thinking"]
        );
    }

    #[test]
    fn an_empty_delta_opens_nothing() {
        let mut transcript = Transcript::new();
        transcript.append_assistant_delta("");
        transcript.append_thinking_delta("");
        assert!(transcript.is_empty());
    }

    #[test]
    fn a_run_of_deltas_asks_for_one_repaint_not_one_each() {
        let mut transcript = Transcript::new();
        for _ in 0..60 {
            transcript.append_thinking_delta("another thought ");
        }
        // Sixty deltas, one outstanding repaint — and it is a streaming one, so
        // the pane may hold it until the flush timer comes round.
        assert!(transcript.is_streaming());
        assert!(transcript.take_dirty());
        assert!(!transcript.take_dirty());
    }

    #[test]
    fn a_structural_change_is_not_held_back_by_the_flush_timer() {
        let mut transcript = Transcript::new();
        transcript.append_assistant_delta("streaming");
        assert!(transcript.is_streaming());
        transcript.add_tool("read", "src/main.rs", "");
        // A tool call arrives at human speed; waiting 50ms to show it would
        // read as a stall rather than as a saving.
        assert!(!transcript.is_streaming());
        assert!(transcript.take_dirty());
    }

    #[test]
    fn a_tool_result_finds_its_call_however_far_back_it_is() {
        let mut transcript = Transcript::new();
        let read = transcript.add_tool("read", "src/main.rs", "path: src/main.rs");
        transcript.append_assistant_delta("Let me also check the tests.");
        transcript.add_tool("read", "tests/mod.rs", "");
        transcript.append_tool_detail(read, "fn main() {}");
        match &transcript.blocks()[read] {
            Block::Tool { detail, .. } => {
                assert_eq!(detail, "path: src/main.rs\nfn main() {}");
            }
            other => panic!("expected a tool block, got {other:?}"),
        }
    }

    #[test]
    fn a_result_for_a_call_that_is_gone_is_dropped_rather_than_fatal() {
        let mut transcript = Transcript::new();
        transcript.add_user("hello", Vec::new());
        // Index past the end, and an index that is not a tool call.
        transcript.append_tool_detail(99, "late result");
        transcript.append_tool_detail(0, "late result");
        assert_eq!(transcript.len(), 1);
    }

    #[test]
    fn folding_a_block_open_is_a_repaint_and_folding_it_again_is_not() {
        let mut transcript = Transcript::new();
        let index = transcript.add_tool("bash", "ls", "a\nb");
        transcript.take_dirty();
        assert!(transcript.set_expanded(index, true));
        assert!(transcript.take_dirty());
        assert!(!transcript.set_expanded(index, true));
        assert!(!transcript.take_dirty());
    }

    #[test]
    fn a_collapsed_reasoning_row_summarises_its_first_real_line() {
        assert_eq!(
            thinking_summary("## Planning\n\nI will start with the router.\n"),
            "Planning I will start with the router."
        );
        // Markers are dropped rather than rendered: this is one line in a row,
        // not a document.
        assert_eq!(thinking_summary("- **first** step\n"), "first step");
        assert_eq!(thinking_summary("\n\n   \n"), "");
    }

    #[test]
    fn a_long_summary_is_clipped_with_an_ellipsis() {
        let long = "word ".repeat(80);
        let summary = thinking_summary(&long);
        assert!(summary.chars().count() <= SUMMARY_LIMIT);
        assert!(summary.ends_with('…'));
    }

    #[test]
    fn a_summary_reads_only_the_head_of_a_streaming_block() {
        // The tail cannot reach the summary, so a block being streamed into
        // costs the same to summarise however long it grows.
        let mut text = "a".repeat(SUMMARY_SCAN);
        text.push_str(" TAIL");
        assert!(!thinking_summary(&text).contains("TAIL"));
    }

    fn session() -> Vec<Value> {
        vec![
            json!({ "role": "user", "content": "add a test" }),
            json!({
                "role": "assistant",
                "content": [
                    { "type": "thinking", "thinking": "Where do the tests live?" },
                    { "type": "text", "text": "I'll look." },
                    {
                        "type": "toolCall",
                        "id": "call-1",
                        "name": "read",
                        "arguments": { "path": "tests/mod.rs" }
                    }
                ]
            }),
            json!({
                "role": "toolResult",
                "toolCallId": "call-1",
                "content": "mod parser;"
            }),
            json!({ "role": "bashExecution", "command": "cargo test", "output": "ok" }),
        ]
    }

    #[test]
    fn a_stored_session_replays_into_the_blocks_it_was_built_from() {
        let mut transcript = Transcript::new();
        transcript.render_messages(&session(), &[]);
        assert_eq!(
            transcript
                .blocks()
                .iter()
                .map(Block::kind)
                .collect::<Vec<_>>(),
            ["user", "thinking", "assistant", "tool", "tool"]
        );
        // The result landed in the call it belongs to, not in a block of its own.
        match &transcript.blocks()[3] {
            Block::Tool { name, detail, .. } => {
                assert_eq!(name, "read");
                assert!(detail.ends_with("mod parser;"), "{detail:?}");
            }
            other => panic!("expected a tool block, got {other:?}"),
        }
        // A bash execution is a tool call whose detail is its output.
        match &transcript.blocks()[4] {
            Block::Tool { name, summary, detail, .. } => {
                assert_eq!(name, "bash");
                assert_eq!(summary, "cargo test");
                assert_eq!(detail, "ok");
            }
            other => panic!("expected a tool block, got {other:?}"),
        }
    }

    #[test]
    fn a_replayed_footer_closes_the_turn_it_belongs_to() {
        let mut transcript = Transcript::new();
        transcript.render_messages(&session(), &[(1, "8s · 512 tokens".into())]);
        let kinds: Vec<&str> = transcript.blocks().iter().map(Block::kind).collect();
        // After the assistant's last part, before the tool result's detail
        // lands on it.
        assert_eq!(
            kinds,
            ["user", "thinking", "assistant", "tool", "footer", "tool"]
        );
    }

    #[test]
    fn a_failed_turn_says_so_in_the_transcript() {
        let messages = vec![json!({
            "role": "assistant",
            "content": [],
            "stopReason": "error",
            "errorMessage": "provider timed out"
        })];
        let mut transcript = Transcript::new();
        transcript.render_messages(&messages, &[]);
        match &transcript.blocks()[0] {
            Block::Info { text, error } => {
                assert!(*error);
                assert!(text.starts_with("Turn failed: "), "{text:?}");
            }
            other => panic!("expected an info block, got {other:?}"),
        }
    }

    #[test]
    fn a_replay_replaces_whatever_was_there() {
        let mut transcript = Transcript::new();
        transcript.add_user("old conversation", Vec::new());
        transcript.render_messages(&session(), &[]);
        assert!(!matches!(
            &transcript.blocks()[0],
            Block::User { text, .. } if text == "old conversation"
        ));
    }
}
