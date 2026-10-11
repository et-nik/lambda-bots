//! Where chat requests go: the worker thread in a live game; nowhere in a replay, whose replies come from the
//! recording; canned answers in tests.

use std::time::Duration;

use lb_chat::{ChatRequest, MapSummary, Reply};
use lb_config::main_config::ChatConfig;

pub enum Job {
    Ask(Box<ChatRequest>),
    /// A map ended: the memory takes it in at once; the model may update its notes once nothing else waits.
    MapEnd(Box<MapSummary>),
    /// New settings.
    Configure(Box<ChatConfig>),
    /// Print the prompt a request would make, without sending it.
    Preview(Box<ChatRequest>),
    /// Print what the bots remember of a player, or forget them.
    Memory {
        query: String,
        forget: bool,
    },
    /// Read `config/chat/` again, and try the provider at once, whatever it answered last.
    Reload,
}

pub trait ChatBackend: Send {
    fn send(&mut self, job: Job);
    /// Replies that came in since the last poll.
    fn poll(&mut self) -> Vec<Reply>;
    /// What the backend is doing, for `lb chat status`: whether requests go first, a line for each thing it tells;
    /// nothing decides by it.
    fn status(&self) -> String;
    /// Stops it, waiting at most `wait` for it to keep what it remembers.
    fn shutdown(&mut self, wait: Duration);
    /// A worker runs.
    fn live(&self) -> bool {
        false
    }
}

/// No worker: chat is off, or this is a replay.
pub struct NullBackend;

impl ChatBackend for NullBackend {
    fn send(&mut self, _job: Job) {}

    fn poll(&mut self) -> Vec<Reply> {
        Vec::new()
    }

    fn status(&self) -> String {
        "no worker".into()
    }

    fn shutdown(&mut self, _wait: Duration) {}
}

/// What a [`FakeBackend`] was sent, kept apart so a test can look after handing the backend over.
#[cfg(test)]
#[derive(Default)]
pub struct Sent {
    pub asked: Vec<ChatRequest>,
    pub summaries: Vec<MapSummary>,
    /// [`Job::Reload`]s.
    pub reloads: usize,
    /// Whether each of the [`Job::Configure`]s had chat on.
    pub enabled: Vec<bool>,
}

/// Tests: answers each request on the next poll with the next canned outcome, the last one over and over.
#[cfg(test)]
pub struct FakeBackend {
    answers: std::collections::VecDeque<lb_chat::Outcome>,
    ready: Vec<Reply>,
    pub sent: std::sync::Arc<std::sync::Mutex<Sent>>,
}

#[cfg(test)]
impl FakeBackend {
    pub fn answering(answers: &[lb_chat::Outcome]) -> FakeBackend {
        FakeBackend {
            answers: answers.iter().cloned().collect(),
            ready: Vec::new(),
            sent: Default::default(),
        }
    }
}

#[cfg(test)]
impl ChatBackend for FakeBackend {
    fn send(&mut self, job: Job) {
        let mut sent = self.sent.lock().unwrap();
        match job {
            Job::Ask(req) => {
                let outcome = if self.answers.len() > 1 {
                    self.answers.pop_front()
                } else {
                    self.answers.front().cloned()
                }
                .unwrap_or(lb_chat::Outcome::Skip);
                self.ready.push(Reply { id: req.id, outcome });
                sent.asked.push(*req);
            }
            Job::MapEnd(s) => sent.summaries.push(*s),
            Job::Reload => sent.reloads += 1,
            Job::Configure(c) => sent.enabled.push(c.enabled),
            _ => {}
        }
    }

    fn poll(&mut self) -> Vec<Reply> {
        std::mem::take(&mut self.ready)
    }

    fn status(&self) -> String {
        "fake".into()
    }

    fn shutdown(&mut self, _wait: Duration) {}

    fn live(&self) -> bool {
        true
    }
}
