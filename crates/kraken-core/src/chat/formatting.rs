//! Formatting helpers for Pi's message payloads.
//!
//! Pure functions that turn Pi's message and tool payloads into the short
//! strings the transcript shows. Shared by the transcript renderer and the
//! session controller that feeds it, so a footer reads the same whether the
//! turn just finished or was replayed off a session file.
//!
//! Everything here is user-visible text, so the functions are specified by
//! their output: the tests below assert exact strings rather than shapes.

use std::collections::BTreeMap;

use serde_json::Value;

/// How much of a tool's arguments or output the expanded view keeps. Tool
/// output is unbounded — a build log, a whole file — and the transcript holds
/// every block for the life of the session.
pub const DETAIL_LIMIT: usize = 4000;

/// One-line summary of an errored assistant message, or `None`.
///
/// Provider error payloads can be pages of repeated JSON; one clipped line
/// keeps the transcript readable while naming the actual failure.
pub fn error_summary(message: &Value) -> Option<String> {
    if message.get("role").and_then(Value::as_str) != Some("assistant")
        || message.get("stopReason").and_then(Value::as_str) != Some("error")
    {
        return None;
    }
    let raw = message
        .get("errorMessage")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .unwrap_or("unknown error");
    let text = collapse_whitespace(raw);
    Some(ellipsize(&text, 300))
}

/// The readable text of a Pi `content` field, which is either a bare string or
/// a list of typed parts. Non-text parts (images, tool calls) contribute
/// nothing: they are shown by blocks of their own.
pub fn content_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.trim().to_string(),
        Value::Array(parts) => parts
            .iter()
            .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
            .map(|part| part.get("text").and_then(Value::as_str).unwrap_or(""))
            .collect::<Vec<_>>()
            .join(" ")
            .trim()
            .to_string(),
        _ => String::new(),
    }
}

/// The argument a tool call is worth naming on its collapsed row: the one the
/// reader would recognise the call by, whitespace-flattened onto one line.
pub fn args_summary(args: &Value) -> String {
    let Some(map) = args.as_object() else {
        return String::new();
    };
    for key in ["command", "path", "file_path", "pattern", "url"] {
        if let Some(text) = map.get(key).and_then(Value::as_str) {
            return ellipsize(&collapse_whitespace(text), 80);
        }
    }
    String::new()
}

/// Full tool arguments for the expanded view.
///
/// Keys come out sorted: `serde_json` is built without `preserve_order`, so
/// object key order is lost at parse time and there is no insertion order left
/// to honour. Pi's argument objects are small, and a stable order at least
/// never reshuffles between two renders of the same call.
pub fn args_detail(args: &Value) -> String {
    match args.as_object() {
        Some(map) if !map.is_empty() => {
            serde_json::to_string_pretty(args).unwrap_or_default()
        }
        _ => String::new(),
    }
}

/// A tool's detail text, trimmed and capped at [`DETAIL_LIMIT`] characters with
/// the cut marked, so a reader can tell a short result from a truncated one.
pub fn clip(text: &str) -> String {
    let text = text.trim();
    if text.chars().count() > DETAIL_LIMIT {
        let head: String = text.chars().take(DETAIL_LIMIT).collect();
        format!("{head}\n… (truncated)")
    } else {
        text.to_string()
    }
}

/// A duration at the precision a reader can act on: tenths while a turn is
/// still countable in seconds, whole seconds up to a minute, then m:ss.
pub fn duration(seconds: f64) -> String {
    let seconds = if seconds.is_nan() { 0.0 } else { seconds.max(0.0) };
    if seconds < 10.0 {
        return format!("{seconds:.1}s");
    }
    if seconds < 60.0 {
        return format!("{seconds:.0}s");
    }
    let whole = seconds as i64;
    let (minutes, secs) = (whole / 60, whole % 60);
    if minutes < 60 {
        return format!("{minutes}:{secs:02}");
    }
    let (hours, minutes) = (minutes / 60, minutes % 60);
    format!("{hours}:{minutes:02}:{secs:02}")
}

/// The footer under a finished reply: how long the turn took, how much it
/// produced, and how fast.
///
/// The two clocks are deliberately separate. `elapsed` is the whole turn — the
/// number the busy row was counting — while the rate is over `generating`
/// alone, the time the model was actually emitting tokens. Dividing by the turn
/// would report the speed of whatever the turn waited on (a slow prefill, a
/// minute-long test run) rather than the speed of the model, which is the only
/// reading of "tokens per second" anyone means. When the two differ enough to
/// make the arithmetic look wrong, the footer shows both rather than leaving
/// the reader to reconcile them.
///
/// `generating` of 0 means nobody timed the generation — the case for a session
/// replayed from disk, where the file records when a request went out and when
/// the reply landed but never when the model started emitting. The footer then
/// stops after the token count rather than printing a rate that would mean
/// something different from the one a live turn shows.
pub fn turn_stats(elapsed: f64, generating: f64, tokens: i64) -> String {
    let mut head = duration(elapsed);
    if generating > 0.0 && generating < 0.8 * elapsed && elapsed - generating >= 1.0 {
        head.push_str(&format!(" ({} generating)", duration(generating)));
    }
    let mut parts = vec![head];
    if tokens > 0 {
        parts.push(format!("{} tok", thousands(tokens)));
        if generating > 0.0 {
            let rate = tokens as f64 / generating;
            parts.push(if rate < 10.0 {
                format!("{rate:.1} tok/s")
            } else {
                format!("{rate:.0} tok/s")
            });
        }
    }
    parts.join(" · ")
}

/// A token count at reading precision: exact under a thousand, then k and M.
/// The readout sits next to a climbing clock in a row nobody stares at, so a
/// digit that changes every second buys nothing.
pub fn token_count(count: i64) -> String {
    if count < 1_000 {
        return count.to_string();
    }
    if count < 1_000_000 {
        let thousands = count as f64 / 1_000.0;
        return if thousands < 10.0 {
            format!("{thousands:.1}k")
        } else {
            format!("{thousands:.0}k")
        };
    }
    format!("{:.1}M", count as f64 / 1_000_000.0)
}

/// The busy row's context readout — `84k / 131k · 62%` — from pi's
/// `contextUsage`, or `None` when there is nothing honest to show.
///
/// Pi omits `contextUsage` when no model or context window is known, and
/// reports `tokens`/`percent` as null in the window between a compaction and
/// the next assistant response that measures the rebuilt context. Both mean
/// "unknown", which the row shows by hiding rather than by holding a stale
/// number that would read as a live one.
///
/// The percent is pi's own when it sends one, since that is the figure its
/// compaction threshold is judged against, and only computed here when it is
/// absent but the two counts are not.
pub fn context_label(usage: Option<&Value>) -> Option<String> {
    let usage = usage?.as_object()?;
    let tokens = usage.get("tokens").and_then(Value::as_f64)?;
    let window = usage.get("contextWindow").and_then(Value::as_f64)?;
    if tokens < 0.0 || window <= 0.0 {
        return None;
    }
    let percent = usage
        .get("percent")
        .and_then(Value::as_f64)
        .unwrap_or(tokens / window * 100.0);
    Some(format!(
        "{} / {} · {percent:.0}%",
        token_count(tokens as i64),
        token_count(window as i64)
    ))
}

/// Footers for a session read back from disk, keyed by the index of the message
/// each one follows.
///
/// A turn runs from a user message to the last assistant message before the
/// next one, which is the same span the live footer covers between agent_start
/// and agent_end. `written[i]` is when pi finished writing message `i`, so the
/// turn's elapsed time is the gap between the user's message landing and its
/// last reply completing. No rate: see [`turn_stats`].
pub fn replay_footers(messages: &[Value], written: &[f64]) -> BTreeMap<usize, String> {
    let mut footers = BTreeMap::new();
    let mut start: Option<f64> = None;
    let mut tokens: i64 = 0;
    let mut last_reply: Option<usize> = None;

    let mut close = |start: Option<f64>, last_reply: Option<usize>, tokens: i64| {
        let (Some(start), Some(reply)) = (start, last_reply) else {
            return;
        };
        let elapsed = written[reply] - start;
        if tokens <= 0 && elapsed < 1.0 {
            // A turn that failed on arrival — an aborted prompt, a rejected
            // request — is written in the same instant it started. "0.0s" is
            // noise under a reply that never happened.
            return;
        }
        footers.insert(reply, turn_stats(elapsed, 0.0, tokens));
    };

    for (index, message) in messages.iter().enumerate() {
        if index >= written.len() {
            break;
        }
        match message.get("role").and_then(Value::as_str) {
            Some("user") => {
                close(start, last_reply, tokens);
                start = Some(written[index]);
                tokens = 0;
                last_reply = None;
            }
            Some("assistant") if start.is_some() => {
                last_reply = Some(index);
                tokens += message
                    .get("usage")
                    .and_then(|usage| usage.get("output"))
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
            }
            _ => {}
        }
    }
    close(start, last_reply, tokens);
    footers
}

/// Every run of whitespace flattened to one space, the way a one-line summary
/// needs it: a tool's `command` can carry newlines and a wrapped shell line.
fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `text` at most `limit` characters, the last one spent on the ellipsis that
/// says something was dropped.
fn ellipsize(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let head: String = text.chars().take(limit - 1).collect();
    format!("{head}…")
}

/// Thousands separators, which the token count in a footer is read at a glance.
fn thousands(count: i64) -> String {
    let digits = count.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    if count < 0 {
        format!("-{out}")
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn reply(tokens: i64) -> Value {
        json!({"role": "assistant", "usage": {"output": tokens}})
    }

    #[test]
    fn the_rate_is_over_generation_time_not_the_whole_turn() {
        // The case that motivates the split: 22s of turn, most of it prompt
        // processing, 16 tokens produced in two thirds of a second. Averaged
        // over the turn this would read 0.7 tok/s and say nothing about the
        // model.
        assert_eq!(
            turn_stats(22.2, 0.66, 16),
            "22s (0.7s generating) · 16 tok · 24 tok/s"
        );
    }

    #[test]
    fn one_clock_when_the_turn_was_all_generation() {
        assert_eq!(turn_stats(3.4, 3.4, 210), "3.4s · 210 tok · 62 tok/s");
    }

    #[test]
    fn a_turn_that_produced_nothing_reports_only_its_time() {
        assert_eq!(turn_stats(12.0, 0.0, 0), "12s");
    }

    #[test]
    fn tool_time_counts_in_the_turn_but_not_in_the_rate() {
        assert_eq!(
            turn_stats(12.0, 4.0, 150),
            "12s (4.0s generating) · 150 tok · 38 tok/s"
        );
    }

    #[test]
    fn a_replayed_turn_stops_after_the_token_count() {
        assert_eq!(turn_stats(10.0, 0.0, 89), "10s · 89 tok");
    }

    #[test]
    fn a_large_reply_gets_thousands_separators() {
        assert_eq!(turn_stats(60.0, 0.0, 12345), "1:00 · 12,345 tok");
    }

    #[test]
    fn durations_step_down_in_precision_as_they_grow() {
        assert_eq!(duration(0.0), "0.0s");
        assert_eq!(duration(-3.0), "0.0s");
        assert_eq!(duration(9.94), "9.9s");
        assert_eq!(duration(10.0), "10s");
        assert_eq!(duration(59.4), "59s");
        assert_eq!(duration(61.0), "1:01");
        assert_eq!(duration(600.0), "10:00");
        assert_eq!(duration(3600.0), "1:00:00");
        assert_eq!(duration(3725.0), "1:02:05");
    }

    #[test]
    fn context_reads_as_used_of_window_and_percent() {
        assert_eq!(
            context_label(Some(
                &json!({"tokens": 84000, "contextWindow": 131072, "percent": 64})
            ))
            .as_deref(),
            Some("84k / 131k · 64%")
        );
    }

    #[test]
    fn small_counts_stay_exact_and_large_ones_go_to_millions() {
        assert_eq!(
            context_label(Some(
                &json!({"tokens": 900, "contextWindow": 2000000, "percent": 0})
            ))
            .as_deref(),
            Some("900 / 2.0M · 0%")
        );
    }

    #[test]
    fn pis_own_percent_wins_over_our_arithmetic() {
        // Pi's figure is the one its compaction threshold is judged against, so
        // the row must not disagree with it over a rounding difference.
        assert_eq!(
            context_label(Some(
                &json!({"tokens": 50000, "contextWindow": 100000, "percent": 62})
            ))
            .as_deref(),
            Some("50k / 100k · 62%")
        );
    }

    #[test]
    fn a_missing_percent_is_computed_rather_than_dropped() {
        assert_eq!(
            context_label(Some(&json!({"tokens": 50000, "contextWindow": 100000}))).as_deref(),
            Some("50k / 100k · 50%")
        );
    }

    #[test]
    fn there_is_no_context_reading_without_a_model_or_a_window() {
        assert_eq!(context_label(None), None);
        assert_eq!(context_label(Some(&json!({}))), None);
        assert_eq!(
            context_label(Some(&json!({"tokens": 100, "contextWindow": 0}))),
            None
        );
    }

    #[test]
    fn the_gap_after_a_compaction_shows_nothing_rather_than_a_stale_number() {
        // Pi reports null tokens between a compaction and the next assistant
        // response. Holding the pre-compaction number there would read as live,
        // and would read as high at exactly the moment it had just dropped.
        assert_eq!(
            context_label(Some(
                &json!({"tokens": null, "contextWindow": 131072, "percent": null})
            )),
            None
        );
    }

    #[test]
    fn a_replayed_turn_reports_time_and_tokens_but_no_rate() {
        let messages = vec![json!({"role": "user"}), reply(89)];
        let footers = replay_footers(&messages, &[100.0, 110.0]);
        assert_eq!(footers, BTreeMap::from([(1, "10s · 89 tok".to_string())]));
    }

    #[test]
    fn a_replayed_turn_ends_at_its_last_reply() {
        // Tool round-trips belong to the turn that started them: one footer, on
        // the final reply, counting every reply's tokens.
        let messages = vec![
            json!({"role": "user"}),
            reply(100),
            json!({"role": "toolResult"}),
            reply(50),
        ];
        let footers = replay_footers(&messages, &[0.0, 3.0, 7.0, 12.0]);
        assert_eq!(footers, BTreeMap::from([(3, "12s · 150 tok".to_string())]));
    }

    #[test]
    fn each_replayed_turn_gets_its_own_footer() {
        let messages = vec![
            json!({"role": "user"}),
            reply(10),
            json!({"role": "user"}),
            reply(20),
        ];
        let footers = replay_footers(&messages, &[0.0, 2.0, 60.0, 64.0]);
        assert_eq!(
            footers,
            BTreeMap::from([
                (1, "2.0s · 10 tok".to_string()),
                (3, "4.0s · 20 tok".to_string()),
            ])
        );
    }

    #[test]
    fn a_turn_that_failed_on_arrival_gets_no_footer() {
        let messages = vec![json!({"role": "user"}), reply(0)];
        assert!(replay_footers(&messages, &[500.0, 500.0]).is_empty());
    }

    #[test]
    fn a_turn_that_burned_time_without_producing_tokens_still_reports_it() {
        let messages = vec![json!({"role": "user"}), reply(0)];
        assert_eq!(
            replay_footers(&messages, &[0.0, 4.6]),
            BTreeMap::from([(1, "4.6s".to_string())])
        );
    }

    #[test]
    fn replies_before_any_prompt_are_not_a_turn() {
        assert!(replay_footers(&[reply(10)], &[5.0]).is_empty());
    }

    #[test]
    fn missing_write_times_are_survivable() {
        let messages = vec![json!({"role": "user"}), reply(10), reply(10)];
        assert_eq!(
            replay_footers(&messages, &[0.0, 1.0]),
            BTreeMap::from([(1, "1.0s · 10 tok".to_string())])
        );
    }

    #[test]
    fn only_an_errored_assistant_message_has_a_summary() {
        assert_eq!(error_summary(&json!({"role": "user"})), None);
        assert_eq!(
            error_summary(&json!({"role": "assistant", "stopReason": "endTurn"})),
            None
        );
        assert_eq!(
            error_summary(&json!({"role": "assistant", "stopReason": "error"})).as_deref(),
            Some("unknown error")
        );
    }

    #[test]
    fn an_error_summary_is_one_flattened_clipped_line() {
        let message = json!({
            "role": "assistant",
            "stopReason": "error",
            "errorMessage": "quota\n  exceeded   for\tthis key",
        });
        assert_eq!(
            error_summary(&message).as_deref(),
            Some("quota exceeded for this key")
        );

        let long = json!({
            "role": "assistant",
            "stopReason": "error",
            "errorMessage": "e".repeat(400),
        });
        let summary = error_summary(&long).expect("errored message");
        assert_eq!(summary.chars().count(), 300);
        assert!(summary.ends_with('…'));
    }

    #[test]
    fn content_text_reads_strings_and_typed_parts_alike() {
        assert_eq!(content_text(&json!("  hello  ")), "hello");
        assert_eq!(
            content_text(&json!([
                {"type": "text", "text": "how many"},
                {"type": "image", "data": "…"},
                {"type": "text", "text": "prompts?"},
            ])),
            "how many prompts?"
        );
        assert_eq!(content_text(&json!(null)), "");
    }

    #[test]
    fn a_tool_summary_names_the_argument_the_call_is_recognised_by() {
        assert_eq!(args_summary(&json!({"command": "ls -la"})), "ls -la");
        // Order of preference, not of the object: a read has both.
        assert_eq!(
            args_summary(&json!({"path": "/tmp/x", "command": "cat"})),
            "cat"
        );
        assert_eq!(args_summary(&json!({"depth": 2})), "");
        assert_eq!(args_summary(&json!("not an object")), "");
    }

    #[test]
    fn a_long_tool_summary_is_clipped_to_one_row() {
        let summary = args_summary(&json!({"command": "x".repeat(200)}));
        assert_eq!(summary.chars().count(), 80);
        assert!(summary.ends_with('…'));
    }

    #[test]
    fn tool_arguments_expand_to_indented_json() {
        assert_eq!(
            args_detail(&json!({"command": "ls -la"})),
            "{\n  \"command\": \"ls -la\"\n}"
        );
        assert_eq!(args_detail(&json!({})), "");
        assert_eq!(args_detail(&json!(null)), "");
    }

    #[test]
    fn non_ascii_arguments_stay_readable_rather_than_escaped() {
        assert_eq!(
            args_detail(&json!({"path": "café/naïve"})),
            "{\n  \"path\": \"café/naïve\"\n}"
        );
    }

    #[test]
    fn detail_is_trimmed_and_a_truncation_says_so() {
        assert_eq!(clip("  total 24\n"), "total 24");
        let clipped = clip(&"y".repeat(DETAIL_LIMIT + 10));
        assert!(clipped.ends_with("\n… (truncated)"));
        assert_eq!(
            clipped.chars().count(),
            DETAIL_LIMIT + "\n… (truncated)".chars().count()
        );
    }

    #[test]
    fn token_counts_lose_precision_as_they_grow() {
        assert_eq!(token_count(0), "0");
        assert_eq!(token_count(999), "999");
        assert_eq!(token_count(1000), "1.0k");
        assert_eq!(token_count(9999), "10.0k");
        assert_eq!(token_count(84000), "84k");
        assert_eq!(token_count(1_500_000), "1.5M");
    }
}
