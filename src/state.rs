use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingAsk {
    pub id: String,
    pub role: String,
    pub chat_id: i64,
    /// Chat message id when sent via sendMessage; absent for guest/inline asks.
    pub message_id: Option<i32>,
    /// Inline message id when sent via answerGuestQuery.
    pub inline_message_id: Option<String>,
    /// Non-empty ⇒ only callback choices complete this ask.
    pub choice_ids: Vec<String>,
    pub created: String,
}

pub struct AskWaiter {
    answer: Mutex<Option<Result<String, String>>>,
    cv: Condvar,
}

pub struct Shared {
    dir: PathBuf,
    offset: Mutex<Option<i64>>,
    /// message_id → ask id
    by_message: Mutex<HashMap<i32, String>>,
    /// inline_message_id → ask id
    by_inline: Mutex<HashMap<String, String>>,
    pending: Mutex<HashMap<String, PendingAsk>>,
    waiters: Mutex<HashMap<String, Arc<AskWaiter>>>,
    /// Guest query ids already answered (answerGuestQuery is one-shot).
    consumed_guest_queries: Mutex<HashSet<String>>,
}

impl Shared {
    pub fn load(dir: &Path) -> Result<Arc<Self>, String> {
        fs::create_dir_all(dir).map_err(|err| format!("creating state dir: {err}"))?;
        let offset_path = dir.join("offset");
        let offset = if offset_path.exists() {
            let text =
                fs::read_to_string(&offset_path).map_err(|err| format!("reading offset: {err}"))?;
            let text = text.trim();
            if text.is_empty() {
                None
            } else {
                Some(
                    text.parse::<i64>()
                        .map_err(|err| format!("parsing offset: {err}"))?,
                )
            }
        } else {
            None
        };
        Ok(Arc::new(Self {
            dir: dir.to_path_buf(),
            offset: Mutex::new(offset),
            by_message: Mutex::new(HashMap::new()),
            by_inline: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
            waiters: Mutex::new(HashMap::new()),
            consumed_guest_queries: Mutex::new(HashSet::new()),
        }))
    }

    pub fn offset(&self) -> Option<i64> {
        *self.offset.lock().expect("offset lock")
    }

    pub fn set_offset(&self, offset: i64) -> Result<(), String> {
        *self.offset.lock().expect("offset lock") = Some(offset);
        fs::write(self.dir.join("offset"), offset.to_string())
            .map_err(|err| format!("writing offset: {err}"))
    }

    /// Returns true the first time `guest_query_id` is seen (caller should answerGuestQuery).
    pub fn consume_guest_query(&self, guest_query_id: &str) -> bool {
        self.consumed_guest_queries
            .lock()
            .expect("consumed_guest_queries lock")
            .insert(guest_query_id.to_string())
    }

    pub fn register_ask(&self, ask: PendingAsk) -> Arc<AskWaiter> {
        let waiter = Arc::new(AskWaiter {
            answer: Mutex::new(None),
            cv: Condvar::new(),
        });
        if let Some(message_id) = ask.message_id {
            self.by_message
                .lock()
                .expect("by_message lock")
                .insert(message_id, ask.id.clone());
        }
        if let Some(inline_id) = &ask.inline_message_id {
            self.by_inline
                .lock()
                .expect("by_inline lock")
                .insert(inline_id.clone(), ask.id.clone());
        }
        self.waiters
            .lock()
            .expect("waiters lock")
            .insert(ask.id.clone(), Arc::clone(&waiter));
        self.pending
            .lock()
            .expect("pending lock")
            .insert(ask.id.clone(), ask);
        waiter
    }

    pub fn complete_ask(&self, ask_id: &str, result: Result<String, String>) -> bool {
        let Some(waiter) = self.waiters.lock().expect("waiters lock").remove(ask_id) else {
            return false;
        };
        if let Some(ask) = self.pending.lock().expect("pending lock").remove(ask_id) {
            if let Some(message_id) = ask.message_id {
                self.by_message
                    .lock()
                    .expect("by_message lock")
                    .remove(&message_id);
            }
            if let Some(inline_id) = &ask.inline_message_id {
                self.by_inline
                    .lock()
                    .expect("by_inline lock")
                    .remove(inline_id);
            }
        }
        *waiter.answer.lock().expect("answer lock") = Some(result);
        waiter.cv.notify_all();
        true
    }

    pub fn pending_ask(&self, ask_id: &str) -> Option<PendingAsk> {
        self.pending
            .lock()
            .expect("pending lock")
            .get(ask_id)
            .cloned()
    }

    pub fn ask_id_for_message(&self, message_id: i32) -> Option<String> {
        self.by_message
            .lock()
            .expect("by_message lock")
            .get(&message_id)
            .cloned()
    }

    pub fn latest_ask_id(&self) -> Option<String> {
        self.pending
            .lock()
            .expect("pending lock")
            .values()
            .filter(|ask| ask.choice_ids.is_empty())
            .max_by_key(|ask| ask.message_id.unwrap_or(0))
            .map(|ask| ask.id.clone())
    }

    pub fn wait_for_answer(waiter: &AskWaiter, timeout: Duration) -> Result<String, String> {
        let deadline = Instant::now() + timeout;
        let mut guard = waiter.answer.lock().expect("answer lock");
        loop {
            if let Some(result) = guard.take() {
                return result;
            }
            let now = Instant::now();
            if now >= deadline {
                return Err("ask timed out".into());
            }
            let (next, wait_result) = waiter.cv.wait_timeout(guard, deadline - now).expect("wait");
            guard = next;
            if wait_result.timed_out() {
                if let Some(result) = guard.take() {
                    return result;
                }
                return Err("ask timed out".into());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("agent-telegram-bot-state-{nanos}"));
        fs::create_dir_all(&dir).expect("create temp state dir");
        dir
    }

    fn sample_ask(id: &str, message_id: i32) -> PendingAsk {
        PendingAsk {
            id: id.into(),
            role: "agent".into(),
            chat_id: 1,
            message_id: Some(message_id),
            inline_message_id: None,
            choice_ids: vec![],
            created: "0".into(),
        }
    }

    #[test]
    fn offset_persists() {
        let dir = temp_dir();
        let state = Shared::load(&dir).unwrap();
        assert_eq!(state.offset(), None);
        state.set_offset(42).unwrap();
        assert_eq!(state.offset(), Some(42));

        let reloaded = Shared::load(&dir).unwrap();
        assert_eq!(reloaded.offset(), Some(42));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn register_complete_and_wait() {
        let dir = temp_dir();
        let state = Shared::load(&dir).unwrap();
        let waiter = state.register_ask(sample_ask("abc", 10));
        assert_eq!(state.ask_id_for_message(10).as_deref(), Some("abc"));
        assert_eq!(state.latest_ask_id().as_deref(), Some("abc"));

        assert!(state.complete_ask("abc", Ok("yes".into())));
        let answer = Shared::wait_for_answer(&waiter, Duration::from_secs(1)).unwrap();
        assert_eq!(answer, "yes");
        assert!(state.ask_id_for_message(10).is_none());
        assert!(state.latest_ask_id().is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn wait_times_out() {
        let dir = temp_dir();
        let state = Shared::load(&dir).unwrap();
        let waiter = state.register_ask(sample_ask("slow", 11));
        let err = Shared::wait_for_answer(&waiter, Duration::from_millis(20)).unwrap_err();
        assert!(err.contains("timed out"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn latest_ask_prefers_highest_message_id() {
        let dir = temp_dir();
        let state = Shared::load(&dir).unwrap();
        let _ = state.register_ask(sample_ask("old", 1));
        let _ = state.register_ask(sample_ask("new", 99));
        assert_eq!(state.latest_ask_id().as_deref(), Some("new"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn latest_ask_skips_choice_asks() {
        let dir = temp_dir();
        let state = Shared::load(&dir).unwrap();
        let mut choice = sample_ask("choice", 50);
        choice.choice_ids = vec!["confirm".into()];
        let _ = state.register_ask(choice);
        let _ = state.register_ask(sample_ask("text", 10));
        assert_eq!(state.latest_ask_id().as_deref(), Some("text"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn consume_guest_query_once() {
        let dir = temp_dir();
        let state = Shared::load(&dir).unwrap();
        assert!(state.consume_guest_query("gq1"));
        assert!(!state.consume_guest_query("gq1"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn register_inline_ask() {
        let dir = temp_dir();
        let state = Shared::load(&dir).unwrap();
        let ask = PendingAsk {
            id: "inl".into(),
            role: "agent".into(),
            chat_id: 1,
            message_id: None,
            inline_message_id: Some("inline-1".into()),
            choice_ids: vec!["confirm".into()],
            created: "0".into(),
        };
        let _ = state.register_ask(ask);
        assert!(state.pending_ask("inl").is_some());
        assert!(state.complete_ask("inl", Ok("confirm".into())));
        assert!(state.pending_ask("inl").is_none());
        let _ = fs::remove_dir_all(&dir);
    }
}
