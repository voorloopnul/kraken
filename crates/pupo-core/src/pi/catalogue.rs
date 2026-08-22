//! Ask pi which models it offers, without a workspace to ask through.
//!
//! The Models settings page needs the catalogue whether or not a session is
//! open, and pi keeps that answer to itself: the models a provider exposes are
//! fetched and cached by pi, merged with what `models.json` declares, and then
//! narrowed to the providers that actually have credentials. Reading those
//! files ourselves would be reimplementing the part of pi most likely to
//! change.
//!
//! So the question is put to pi itself. A short-lived
//! `pi --mode rpc --no-session` in the user's home folder answers
//! `get_available_models` and is stopped as soon as it has — no session file, no
//! workspace, nothing left running.

use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::pi::rpc::{AgentRecord, Launch, PiAgent};

/// How long to wait for the answer.
///
/// Generous, because a first run may be fetching a provider's model list over
/// the network, and this blocks nothing: the page fills in when it arrives.
pub const TIMEOUT: Duration = Duration::from_secs(20);

/// The callback, once. Boxed because it is a closure and `FnOnce` because it is
/// consumed by the one call it gets.
type Answer = Option<Box<dyn FnOnce(Vec<Value>) + Send>>;

/// Held across the worker and the guard's own deadline.
type Shared<T> = Arc<Mutex<T>>;

/// A model-list callback that runs exactly once, and runs for certain.
///
/// An answer can go missing without a word: a pi that dies drops the callbacks
/// waiting on it, and one that never starts never had them. A page waiting on
/// this would sit on "asking…" for the rest of the run, so the guard answers
/// with an empty list itself once the deadline passes — whichever comes first
/// wins, and the loser is ignored.
pub struct DeliverOnce {
    answered: Arc<Mutex<bool>>,
    callback: Shared<Answer>,
    deadline: Instant,
}

impl DeliverOnce {
    pub fn new(callback: impl FnOnce(Vec<Value>) + Send + 'static) -> Self {
        Self::with_timeout(callback, TIMEOUT)
    }

    pub fn with_timeout(
        callback: impl FnOnce(Vec<Value>) + Send + 'static,
        timeout: Duration,
    ) -> Self {
        Self {
            answered: Arc::new(Mutex::new(false)),
            callback: Arc::new(Mutex::new(Some(Box::new(callback)))),
            deadline: Instant::now() + timeout,
        }
    }

    /// Deliver `models`, unless something already answered.
    pub fn deliver(&self, models: Vec<Value>) -> bool {
        let mut answered = self.answered.lock().unwrap_or_else(|e| e.into_inner());
        if *answered {
            return false;
        }
        *answered = true;
        drop(answered);
        let taken = self
            .callback
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        if let Some(callback) = taken {
            callback(models);
        }
        true
    }

    pub fn answered(&self) -> bool {
        *self.answered.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn expired(&self) -> bool {
        Instant::now() >= self.deadline
    }

    /// Answer with an empty list if the deadline has passed and nothing else
    /// has. Call it from whatever the owner already polls on.
    pub fn expire(&self) -> bool {
        self.expired() && self.deliver(Vec::new())
    }

    pub fn time_left(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }
}

/// The models a `get_available_models` response carried.
pub fn models_from(response: &Value) -> Vec<Value> {
    response
        .get("data")
        .and_then(|data| data.get("models"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// One throwaway pi, asked for its model list.
///
/// Answers exactly once — with the models, or with an empty list when pi could
/// not be started, died first, or took too long. The process is stopped before
/// the answer is handed over: the caller may put a dialog on screen, and this
/// one has nothing left to do either way.
pub fn query_models(timeout: Duration) -> Vec<Value> {
    // Home, not a workspace: this pi is asked one question about global
    // configuration, and no folder it could be started in makes that a better
    // answer — while a project folder would drag in project settings.
    let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    let mut agent = PiAgent::new(Launch::new(home).ephemeral());

    let request = match agent.get_available_models() {
        Ok(id) => id,
        Err(_) => {
            agent.stop();
            return Vec::new();
        }
    };

    let deadline = Instant::now() + timeout;
    let mut models = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        match agent.events().recv_timeout(left) {
            Ok(AgentRecord::Response { id, value }) if id == request => {
                models = models_from(&value);
                break;
            }
            // The process died or could not start; there is no answer coming.
            Ok(AgentRecord::Failed(_)) | Ok(AgentRecord::Finished { .. }) => break,
            Ok(_) => continue,
            Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    agent.stop();
    models
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::mpsc;

    #[test]
    fn the_answer_is_delivered_once() {
        let (sender, received) = mpsc::channel();
        let guard = DeliverOnce::new(move |models| {
            let _ = sender.send(models);
        });
        assert!(guard.deliver(vec![json!({ "id": "a" })]));
        // The second call is ignored, and so is a timeout that follows a real
        // answer — a page rebuilt from an empty list after it had the models
        // would be the fetch undoing itself.
        assert!(!guard.deliver(vec![json!({ "id": "b" })]));
        assert!(guard.answered());
        let answers: Vec<Vec<Value>> = received.try_iter().collect();
        assert_eq!(answers, vec![vec![json!({ "id": "a" })]]);
    }

    #[test]
    fn an_answer_that_never_comes_is_an_empty_one() {
        let (sender, received) = mpsc::channel();
        let guard = DeliverOnce::with_timeout(
            move |models| {
                let _ = sender.send(models);
            },
            Duration::from_millis(0),
        );
        // Nothing yet, and nothing is lost by asking early.
        assert!(guard.expired());
        assert!(guard.expire());
        assert_eq!(received.try_iter().collect::<Vec<_>>(), vec![Vec::<Value>::new()]);
    }

    #[test]
    fn a_deadline_that_has_not_passed_delivers_nothing() {
        let (sender, received) = mpsc::channel();
        let guard = DeliverOnce::with_timeout(
            move |models| {
                let _ = sender.send(models);
            },
            Duration::from_secs(60),
        );
        assert!(!guard.expire());
        assert!(!guard.answered());
        assert!(guard.time_left() > Duration::from_secs(50));
        assert!(received.try_iter().next().is_none());
    }

    #[test]
    fn expiring_after_a_real_answer_changes_nothing() {
        let (sender, received) = mpsc::channel();
        let guard = DeliverOnce::with_timeout(
            move |models| {
                let _ = sender.send(models);
            },
            Duration::from_millis(0),
        );
        guard.deliver(vec![json!({ "id": "a" })]);
        assert!(!guard.expire());
        assert_eq!(
            received.try_iter().collect::<Vec<_>>(),
            vec![vec![json!({ "id": "a" })]]
        );
    }

    #[test]
    fn a_models_reply_is_unwrapped_and_anything_else_is_empty() {
        assert_eq!(
            models_from(&json!({ "data": { "models": [{ "id": "a" }] } })).len(),
            1
        );
        assert!(models_from(&json!({ "data": {} })).is_empty());
        assert!(models_from(&json!({})).is_empty());
        // A reply whose models are not a list is not a list of models.
        assert!(models_from(&json!({ "data": { "models": "all of them" } })).is_empty());
    }
}
