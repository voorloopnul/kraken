//! Pi's session files, read straight off disk.
//!
//! Each session is a JSONL file whose first line is a header —
//! `{"type": "session", "id": …, "timestamp": …, "cwd": …}` — followed by event
//! lines (`{"type": "message", …}` among others). The files are grouped in one
//! directory per project, named after a munged cwd; rather than reproducing the
//! munging, directories are matched by what their headers say their cwd is.
//!
//! Reading them here is what makes browsing history free: pi writes the same
//! message objects to disk that it returns over RPC, so a past session renders
//! without starting an agent for it.

use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::pi::config;
use crate::state;

/// The longest title kept; anything past it is elided.
const TITLE_LIMIT: usize = 60;

/// One session file, summarized for the history list.
#[derive(Debug, Clone, PartialEq)]
pub struct PiSession {
    pub path: PathBuf,
    pub session_id: String,
    /// When the session started, in epoch seconds. Formatting it for a reader
    /// (local time, "yesterday", …) belongs to whoever shows it.
    pub started: f64,
    /// The header's own timestamp, kept verbatim for anyone who wants it.
    pub timestamp: String,
    /// The first user message, or `(empty session)`.
    pub title: String,
    pub message_count: usize,
}

/// Where pi keeps its sessions.
pub fn sessions_root() -> PathBuf {
    config::agent_dir().join("sessions")
}

/// The session ids the user has hidden from the history.
pub fn archived_ids() -> HashSet<String> {
    state::string_list("archived_sessions").into_iter().collect()
}

/// Hide a session from the history without deleting its file on disk.
pub fn archive_session(session_id: &str) {
    let mut archived: Vec<String> = archived_ids().into_iter().collect();
    if archived.iter().any(|id| id == session_id) {
        return;
    }
    archived.push(session_id.to_string());
    archived.sort();
    state::set("archived_sessions", json!(archived));
}

/// The session ids the user has pinned to the top of the history.
pub fn pinned_ids() -> HashSet<String> {
    state::string_list("pinned_sessions").into_iter().collect()
}

/// Whether a pinned session keeps its own place above the rest.
///
/// Kept sorted so the stored list is stable between writes; the order a reader
/// sees comes from the listing, not from the order things were pinned.
pub fn pin_session(session_id: &str) {
    let mut pinned: Vec<String> = pinned_ids().into_iter().collect();
    if pinned.iter().any(|id| id == session_id) {
        return;
    }
    pinned.push(session_id.to_string());
    pinned.sort();
    state::set("pinned_sessions", json!(pinned));
}

/// Drop a session back among the rest. Unpinning something that was never
/// pinned is not an error — the end state is what was asked for either way.
pub fn unpin_session(session_id: &str) {
    let mut pinned: Vec<String> = pinned_ids().into_iter().collect();
    pinned.retain(|id| id != session_id);
    pinned.sort();
    state::set("pinned_sessions", json!(pinned));
}

/// Permanently remove a session's file from disk.
pub fn delete_session(path: impl AsRef<Path>) {
    let _ = fs::remove_file(path);
}

/// All Pi sessions recorded for the project folder `cwd`, newest first, with
/// the archived ones omitted.
pub fn sessions_for(cwd: &str) -> Vec<PiSession> {
    list_sessions(&sessions_root(), cwd, &archived_ids())
}

/// The same listing over an explicit root and archive set — the whole of the
/// work, with nothing global in it.
pub fn list_sessions(root: &Path, cwd: &str, archived: &HashSet<String>) -> Vec<PiSession> {
    let target = resolve(cwd);
    let mut sessions = Vec::new();
    let Ok(entries) = fs::read_dir(root) else {
        return sessions;
    };
    for entry in entries.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        let Ok(files) = fs::read_dir(entry.path()) else {
            continue;
        };
        for file in files.flatten() {
            let path = file.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                continue;
            }
            let Some(header) = read_header(&path) else {
                continue;
            };
            if header.get("cwd").and_then(Value::as_str) != Some(target.as_str()) {
                continue;
            }
            let session = load_session(&path, &header);
            if archived.contains(&session.session_id) {
                continue;
            }
            sessions.push(session);
        }
    }
    // Newest first, and by path where two sessions share a timestamp, so the
    // order does not change between two readings of the same directory.
    sessions.sort_by(|left, right| {
        right
            .started
            .partial_cmp(&left.started)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.path.cmp(&right.path))
    });
    sessions
}

/// The absolute path a session header would have recorded for `cwd`.
fn resolve(cwd: &str) -> String {
    let expanded = match cwd.strip_prefix("~/") {
        Some(rest) => dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(rest),
        None => PathBuf::from(cwd),
    };
    // A workspace whose folder is gone still has sessions worth listing, so a
    // path that cannot be canonicalized is used as it was given.
    fs::canonicalize(&expanded)
        .unwrap_or(expanded)
        .to_string_lossy()
        .into_owned()
}

/// The session header, or `None` when the first line is not one.
fn read_header(path: &Path) -> Option<Value> {
    let file = fs::File::open(path).ok()?;
    let mut line = String::new();
    BufReader::new(file).read_line(&mut line).ok()?;
    let header: Value = serde_json::from_str(line.trim()).ok()?;
    (header.get("type").and_then(Value::as_str) == Some("session")).then_some(header)
}

fn load_session(path: &Path, header: &Value) -> PiSession {
    let mut title = String::new();
    let mut count = 0usize;
    if let Ok(file) = fs::File::open(path) {
        for line in BufReader::new(file).lines().skip(1).map_while(Result::ok) {
            let Ok(event) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if event.get("type").and_then(Value::as_str) != Some("message") {
                continue;
            }
            let message = event.get("message").cloned().unwrap_or(Value::Null);
            let role = message.get("role").and_then(Value::as_str).unwrap_or("");
            if role != "user" && role != "assistant" {
                continue;
            }
            count += 1;
            if title.is_empty() && role == "user" {
                title = collapse(&content_text(message.get("content")));
            }
        }
    }

    let timestamp = header
        .get("timestamp")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    PiSession {
        path: path.to_path_buf(),
        session_id: header
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| {
                path.file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
                    .unwrap_or_default()
            }),
        started: parse_timestamp(&timestamp).unwrap_or(0.0),
        timestamp,
        title: if title.is_empty() {
            "(empty session)".to_string()
        } else {
            elide(&title, TITLE_LIMIT)
        },
        message_count: count,
    }
}

/// The text parts of a message's content, whatever shape it came in.
pub fn content_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.trim().to_string(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
            .map(|part| part.get("text").and_then(Value::as_str).unwrap_or_default())
            .collect::<Vec<_>>()
            .join(" ")
            .trim()
            .to_string(),
        _ => String::new(),
    }
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Clip to `limit` characters, marking that something was cut.
pub fn elide(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let head: String = text.chars().take(limit.saturating_sub(1)).collect();
    format!("{head}…")
}

// ---------------------------------------------------------------------------
// Transcripts
// ---------------------------------------------------------------------------

/// A session as it was stored: its messages, when each was written, the model
/// it last used, and its last thinking level.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Transcript {
    /// The message objects verbatim — pi writes the same ones to disk that it
    /// returns over RPC, so they render identically without reshaping.
    pub messages: Vec<Value>,
    /// For each message, the epoch second at which pi wrote the record. For an
    /// assistant message that is when the reply finished; the message's own
    /// `timestamp` is when its request went out, so the pair brackets the
    /// response. (Nothing on disk marks when the model started emitting.)
    pub written: Vec<f64>,
    /// `{provider, id}` from the most recent model_change or assistant message.
    /// There is no friendly name on disk, so a footer shows the id until the
    /// agent later resolves it.
    pub model: Option<Value>,
    pub thinking_level: Option<String>,
}

/// Read a session's stored messages — no live pi process involved.
pub fn read_transcript(path: impl AsRef<Path>) -> Transcript {
    let mut transcript = Transcript::default();
    let Ok(file) = fs::File::open(path.as_ref()) else {
        return transcript;
    };
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(event) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        match event.get("type").and_then(Value::as_str).unwrap_or("") {
            "message" => {
                let Some(message) = event.get("message").filter(|m| m.is_object()) else {
                    continue;
                };
                transcript.written.push(written_at(&event, message));
                if message.get("role").and_then(Value::as_str) == Some("assistant") {
                    if let Some(provider) = message.get("provider").and_then(Value::as_str) {
                        transcript.model = Some(json!({
                            "provider": provider,
                            "id": message.get("model").cloned().unwrap_or(Value::Null),
                        }));
                    }
                }
                transcript.messages.push(message.clone());
            }
            "model_change" => {
                if let Some(provider) = event.get("provider").and_then(Value::as_str) {
                    transcript.model = Some(json!({
                        "provider": provider,
                        "id": event.get("modelId").cloned().unwrap_or(Value::Null),
                    }));
                }
            }
            "thinking_level_change" => {
                if let Some(level) = event.get("thinkingLevel").and_then(Value::as_str) {
                    transcript.thinking_level = Some(level.to_string());
                }
            }
            _ => {}
        }
    }
    transcript
}

/// When pi committed this record, in epoch seconds. Falls back to the message's
/// own start time, so a record without a parseable timestamp costs the turn its
/// footer rather than shifting every later one.
pub fn written_at(event: &Value, message: &Value) -> f64 {
    if let Some(stamp) = event.get("timestamp").and_then(Value::as_str) {
        if let Some(seconds) = parse_timestamp(stamp) {
            return seconds;
        }
    }
    // The message's own timestamp is epoch milliseconds.
    message
        .get("timestamp")
        .and_then(Value::as_f64)
        .map(|millis| millis / 1000.0)
        .unwrap_or(0.0)
}

/// Epoch seconds for an ISO-8601 timestamp of the shape pi writes
/// (`2026-07-22T14:05:18.076Z`), with an explicit offset also accepted.
///
/// A hand-rolled parser rather than a date crate: this is the only date the app
/// reads, and all it needs is an instant to sort and subtract.
pub fn parse_timestamp(text: &str) -> Option<f64> {
    let text = text.trim();
    let (date, rest) = text.split_once(['T', ' '])?;
    let mut date = date.split('-');
    let year: i64 = date.next()?.parse().ok()?;
    let month: i64 = date.next()?.parse().ok()?;
    let day: i64 = date.next()?.parse().ok()?;
    if date.next().is_some() {
        return None;
    }

    // The offset, if any, ends the string; what is left is the clock time.
    let (clock, offset) = match rest.strip_suffix(['Z', 'z']) {
        Some(clock) => (clock, 0.0),
        None => match rest.rfind(['+', '-']) {
            Some(index) => {
                let (clock, zone) = rest.split_at(index);
                let sign = if zone.starts_with('-') { -1.0 } else { 1.0 };
                let mut parts = zone[1..].split(':');
                let hours: f64 = parts.next()?.parse().ok()?;
                let minutes: f64 = parts.next().unwrap_or("0").parse().ok()?;
                (clock, sign * (hours * 3600.0 + minutes * 60.0))
            }
            // No zone at all: read as UTC, which is what pi writes anyway.
            None => (rest, 0.0),
        },
    };
    let mut clock = clock.split(':');
    let hour: f64 = clock.next()?.parse().ok()?;
    let minute: f64 = clock.next()?.parse().ok()?;
    let second: f64 = clock.next().unwrap_or("0").parse().ok()?;

    let days = days_from_civil(year, month, day)? as f64;
    Some(days * 86400.0 + hour * 3600.0 + minute * 60.0 + second - offset)
}

/// Days between 1970-01-01 and the given civil date (Howard Hinnant's
/// algorithm), which is all the calendar arithmetic this module needs.
fn days_from_civil(year: i64, month: i64, day: i64) -> Option<i64> {
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some(era * 146_097 + day_of_era - 719_468)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch {
        root: PathBuf,
    }

    impl Scratch {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!("pupo-pi-sessions-{name}"));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).expect("a scratch sessions root");
            Self { root }
        }

        /// Write one session file the way pi writes them: a header line, then
        /// events.
        fn write(&self, folder: &str, name: &str, lines: &[String]) -> PathBuf {
            let dir = self.root.join(folder);
            fs::create_dir_all(&dir).expect("a project directory");
            let path = dir.join(name);
            fs::write(&path, format!("{}\n", lines.join("\n"))).expect("a session file");
            path
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn header(id: &str, timestamp: &str, cwd: &str) -> String {
        json!({"type": "session", "version": 3, "id": id, "timestamp": timestamp, "cwd": cwd})
            .to_string()
    }

    fn user(text: &str, timestamp: &str) -> String {
        json!({
            "type": "message", "timestamp": timestamp,
            "message": {"role": "user", "content": [{"type": "text", "text": text}]}
        })
        .to_string()
    }

    fn assistant(text: &str, timestamp: &str) -> String {
        json!({
            "type": "message", "timestamp": timestamp,
            "message": {
                "role": "assistant",
                "provider": "anthropic",
                "model": "claude-opus-4-8",
                "content": [{"type": "text", "text": text}],
                "usage": {"output": 120},
            }
        })
        .to_string()
    }

    #[test]
    fn sessions_are_matched_by_the_cwd_their_header_records() {
        let scratch = Scratch::new("cwd");
        scratch.write(
            "--home-pascal-Workspace-pupo--",
            "a.jsonl",
            &[
                header("a", "2026-07-22T14:05:18.076Z", "/home/pascal/Workspace/pupo"),
                user("port the rpc client", "2026-07-22T14:05:19.000Z"),
            ],
        );
        scratch.write(
            "--home-pascal-Workspace-alpine--",
            "b.jsonl",
            &[header("b", "2026-07-22T15:00:00.000Z", "/home/pascal/Workspace/alpine")],
        );
        let found = list_sessions(&scratch.root, "/home/pascal/Workspace/pupo", &HashSet::new());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].session_id, "a");
        assert_eq!(found[0].title, "port the rpc client");
        // The munged directory name is never parsed, so a session filed under
        // the "wrong" folder is still found by its header.
        let stray = list_sessions(&scratch.root, "/home/pascal/Workspace/alpine", &HashSet::new());
        assert_eq!(stray.len(), 1);
        assert_eq!(stray[0].session_id, "b");
    }

    #[test]
    fn the_newest_session_is_listed_first() {
        let scratch = Scratch::new("order");
        for (id, stamp) in [
            ("older", "2026-07-20T09:00:00.000Z"),
            ("newest", "2026-07-22T14:05:18.076Z"),
            ("middle", "2026-07-21T23:59:59.000Z"),
        ] {
            scratch.write("--tmp--", &format!("{id}.jsonl"), &[header(id, stamp, "/tmp")]);
        }
        let ids: Vec<String> = list_sessions(&scratch.root, "/tmp", &HashSet::new())
            .into_iter()
            .map(|session| session.session_id)
            .collect();
        assert_eq!(ids, ["newest", "middle", "older"]);
    }

    #[test]
    fn an_archived_session_is_hidden_without_its_file_going_anywhere() {
        let scratch = Scratch::new("archived");
        let path = scratch.write(
            "--tmp--",
            "a.jsonl",
            &[header("a", "2026-07-22T14:05:18.076Z", "/tmp")],
        );
        scratch.write(
            "--tmp--",
            "b.jsonl",
            &[header("b", "2026-07-22T15:05:18.076Z", "/tmp")],
        );
        let archived: HashSet<String> = ["a".to_string()].into_iter().collect();
        let ids: Vec<String> = list_sessions(&scratch.root, "/tmp", &archived)
            .into_iter()
            .map(|session| session.session_id)
            .collect();
        assert_eq!(ids, ["b"]);
        assert!(path.exists(), "archiving must not touch the file");
        // Deleting is the other half, and that one does remove it.
        delete_session(&path);
        assert!(!path.exists());
        delete_session(&path); // a second delete is not an error
    }

    #[test]
    fn a_session_is_summarized_by_its_first_user_message() {
        let scratch = Scratch::new("title");
        scratch.write(
            "--tmp--",
            "a.jsonl",
            &[
                header("a", "2026-07-22T14:05:18.076Z", "/tmp"),
                json!({"type": "model_change", "provider": "anthropic"}).to_string(),
                user("  tell me   about\nthe vehicles  ", "2026-07-22T14:05:19.000Z"),
                assistant("Certainly.", "2026-07-22T14:05:24.728Z"),
                user("and the second question", "2026-07-22T14:06:00.000Z"),
            ],
        );
        let session = &list_sessions(&scratch.root, "/tmp", &HashSet::new())[0];
        // Whitespace collapsed, and the *first* prompt wins.
        assert_eq!(session.title, "tell me about the vehicles");
        // Only user and assistant messages count as conversation.
        assert_eq!(session.message_count, 3);
    }

    #[test]
    fn a_session_with_nothing_in_it_says_so() {
        let scratch = Scratch::new("empty");
        scratch.write(
            "--tmp--",
            "a.jsonl",
            &[header("a", "2026-07-22T14:05:18.076Z", "/tmp")],
        );
        let session = &list_sessions(&scratch.root, "/tmp", &HashSet::new())[0];
        assert_eq!(session.title, "(empty session)");
        assert_eq!(session.message_count, 0);
    }

    #[test]
    fn a_long_first_prompt_is_elided_rather_than_wrapped() {
        let scratch = Scratch::new("elide");
        let prompt = "x".repeat(200);
        scratch.write(
            "--tmp--",
            "a.jsonl",
            &[
                header("a", "2026-07-22T14:05:18.076Z", "/tmp"),
                user(&prompt, "2026-07-22T14:05:19.000Z"),
            ],
        );
        let session = &list_sessions(&scratch.root, "/tmp", &HashSet::new())[0];
        assert_eq!(session.title.chars().count(), TITLE_LIMIT);
        assert!(session.title.ends_with('…'));
    }

    #[test]
    fn a_file_that_is_not_a_session_is_passed_over() {
        let scratch = Scratch::new("garbage");
        scratch.write("--tmp--", "notes.txt", &["not json at all".to_string()]);
        scratch.write("--tmp--", "half.jsonl", &["{ truncated".to_string()]);
        scratch.write(
            "--tmp--",
            "other.jsonl",
            &[json!({"type": "message"}).to_string()],
        );
        assert!(list_sessions(&scratch.root, "/tmp", &HashSet::new()).is_empty());
        // A root that does not exist is an empty history, not a failure.
        assert!(list_sessions(Path::new("/nope/nothing"), "/tmp", &HashSet::new()).is_empty());
    }

    #[test]
    fn a_transcript_comes_back_with_the_times_beside_the_messages() {
        let scratch = Scratch::new("transcript");
        let path = scratch.write(
            "--tmp--",
            "a.jsonl",
            &[
                header("a", "2026-07-22T14:05:18.076Z", "/tmp"),
                json!({"type": "model_change", "provider": "openai-codex", "modelId": "gpt-5.6"})
                    .to_string(),
                json!({"type": "thinking_level_change", "thinkingLevel": "medium"}).to_string(),
                user("hello", "2026-07-22T14:05:19.000Z"),
                assistant("hi", "2026-07-22T14:05:24.500Z"),
            ],
        );
        let transcript = read_transcript(&path);
        assert_eq!(transcript.messages.len(), 2);
        assert_eq!(transcript.messages[0]["role"], "user");
        // The gap between the two brackets the reply, to the half second.
        let elapsed = transcript.written[1] - transcript.written[0];
        assert!((elapsed - 5.5).abs() < 0.001, "elapsed was {elapsed}");
        // The assistant message names the model that answered, which is more
        // current than the model_change that preceded it.
        assert_eq!(
            transcript.model,
            Some(json!({"provider": "anthropic", "id": "claude-opus-4-8"}))
        );
        assert_eq!(transcript.thinking_level.as_deref(), Some("medium"));
    }

    #[test]
    fn a_record_pi_never_timestamped_falls_back_to_the_message_itself() {
        let event = json!({"type": "message", "timestamp": "not a date"});
        let message = json!({"role": "user", "timestamp": 1784729118139i64});
        assert!((written_at(&event, &message) - 1784729118.139).abs() < 0.001);
        // Nothing usable anywhere is 0, which costs one footer rather than
        // shifting every later turn.
        assert_eq!(written_at(&json!({}), &json!({})), 0.0);
    }

    #[test]
    fn a_missing_transcript_reads_as_an_empty_one() {
        let transcript = read_transcript("/nope/nothing/here.jsonl");
        assert_eq!(transcript, Transcript::default());
    }

    #[test]
    fn timestamps_are_read_as_the_instant_they_name() {
        // 2026-07-22T14:05:18.076Z, checked against `date -u -d ... +%s`.
        assert_eq!(parse_timestamp("2026-07-22T14:05:18.076Z"), Some(1784729118.076));
        assert_eq!(parse_timestamp("1970-01-01T00:00:00Z"), Some(0.0));
        // An offset is subtracted, so both spellings name the same instant.
        assert_eq!(
            parse_timestamp("2026-07-22T16:05:18.076+02:00"),
            parse_timestamp("2026-07-22T14:05:18.076Z")
        );
        assert_eq!(
            parse_timestamp("2026-07-22T09:05:18.076-05:00"),
            parse_timestamp("2026-07-22T14:05:18.076Z")
        );
        // A leap day, which the calendar arithmetic has to get right.
        assert_eq!(parse_timestamp("2024-02-29T00:00:00Z"), Some(1709164800.0));
        for broken in ["", "yesterday", "2026-07-22", "2026-13-01T00:00:00Z"] {
            assert_eq!(parse_timestamp(broken), None, "{broken} is not a timestamp");
        }
    }

    #[test]
    fn message_content_reads_the_same_whatever_shape_it_arrives_in() {
        assert_eq!(content_text(Some(&json!("  plain  "))), "plain");
        assert_eq!(
            content_text(Some(&json!([
                {"type": "thinking", "thinking": "hmm"},
                {"type": "text", "text": "one"},
                {"type": "text", "text": "two"},
            ]))),
            "one two"
        );
        assert_eq!(content_text(None), "");
        assert_eq!(content_text(Some(&Value::Null)), "");
    }
}
