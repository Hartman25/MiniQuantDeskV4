//! Execution simulator harness for adversarial broker behavior.

use std::collections::VecDeque;

use mqk_execution::BrokerEvent;

#[derive(Debug, Clone)]
pub enum SimAction {
    Emit(BrokerEvent),
    Duplicate(usize),
    Drop,
    Reorder,
}

#[derive(Debug, Default)]
pub struct ExecutionSimulator {
    queue: VecDeque<BrokerEvent>,
}

impl ExecutionSimulator {
    pub fn new(events: Vec<BrokerEvent>) -> Self {
        Self {
            queue: events.into(),
        }
    }

    pub fn duplicate_front(&mut self, times: usize) {
        if let Some(front) = self.queue.front().cloned() {
            for _ in 0..times {
                self.queue.push_front(front.clone());
            }
        }
    }

    pub fn reverse_all(&mut self) {
        let mut v: Vec<_> = self.queue.drain(..).collect();
        v.reverse();
        self.queue = v.into();
    }

    pub fn pop_all(&mut self) -> Vec<BrokerEvent> {
        self.queue.drain(..).collect()
    }
}