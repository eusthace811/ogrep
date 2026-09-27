//! Bounded FIFO admission. A permit owns a slot until its response is disposed.
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rejected {
    Full,
    Timeout,
}

#[derive(Debug, serde::Serialize)]
pub struct Status {
    pub active: usize,
    pub queued: usize,
    pub capacity: usize,
    pub queue_capacity: usize,
}

struct State {
    active: usize,
    queue: VecDeque<u64>,
    next: u64,
}

pub struct Gate {
    capacity: usize,
    queue_capacity: usize,
    state: Mutex<State>,
    changed: Condvar,
}

pub struct Permit {
    gate: Arc<Gate>,
}

impl Gate {
    pub fn new(capacity: usize, queue_capacity: usize) -> Arc<Self> {
        assert!(capacity > 0);
        Arc::new(Self {
            capacity,
            queue_capacity,
            state: Mutex::new(State {
                active: 0,
                queue: VecDeque::new(),
                next: 0,
            }),
            changed: Condvar::new(),
        })
    }

    pub fn status(&self) -> Status {
        let state = self.state.lock().unwrap();
        Status {
            active: state.active,
            queued: state.queue.len(),
            capacity: self.capacity,
            queue_capacity: self.queue_capacity,
        }
    }

    pub fn acquire(self: &Arc<Self>, timeout: Duration) -> Result<Permit, Rejected> {
        let start = Instant::now();
        let mut state = self.state.lock().unwrap();
        if state.active < self.capacity && state.queue.is_empty() {
            state.active += 1;
            return Ok(Permit {
                gate: Arc::clone(self),
            });
        }
        if state.queue.len() >= self.queue_capacity {
            return Err(Rejected::Full);
        }
        let ticket = state.next;
        state.next = state.next.wrapping_add(1);
        state.queue.push_back(ticket);
        self.changed.notify_all();
        loop {
            let remaining = timeout.saturating_sub(start.elapsed());
            if remaining.is_zero() {
                state.queue.retain(|&queued| queued != ticket);
                self.changed.notify_all();
                return Err(Rejected::Timeout);
            }
            if state.active < self.capacity && state.queue.front() == Some(&ticket) {
                state.queue.pop_front();
                state.active += 1;
                self.changed.notify_all();
                return Ok(Permit {
                    gate: Arc::clone(self),
                });
            }
            state = self.changed.wait_timeout(state, remaining).unwrap().0;
        }
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        let mut state = self.gate.state.lock().unwrap();
        state.active -= 1;
        self.gate.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn queued(gate: &Gate, count: usize) {
        let start = Instant::now();
        let mut state = gate.state.lock().unwrap();
        while state.queue.len() != count {
            let remaining = Duration::from_secs(5).saturating_sub(start.elapsed());
            assert!(!remaining.is_zero(), "waiter did not enter queue");
            state = gate.changed.wait_timeout(state, remaining).unwrap().0;
        }
    }

    #[test]
    fn bounds_queue_and_admits_waiters_in_order() {
        let gate = Gate::new(1, 2);
        let held = gate.acquire(Duration::ZERO).unwrap();
        let mut receivers = Vec::new();
        let mut workers = Vec::new();
        for count in 1..=2 {
            let gate_copy = Arc::clone(&gate);
            let (tx, rx) = mpsc::channel();
            workers.push(std::thread::spawn(move || {
                tx.send(gate_copy.acquire(Duration::from_secs(5)).unwrap())
                    .unwrap();
            }));
            queued(&gate, count);
            receivers.push(rx);
        }
        assert_eq!(gate.acquire(Duration::ZERO).err(), Some(Rejected::Full));
        assert_eq!(gate.status().active, 1);
        drop(held);
        let first = receivers[0].recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(receivers[1].try_recv().is_err());
        drop(first);
        drop(receivers[1].recv_timeout(Duration::from_secs(5)).unwrap());
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!((gate.status().active, gate.status().queued), (0, 0));
    }

    #[test]
    fn deadline_removes_waiter_and_release_restores_capacity() {
        let gate = Gate::new(1, 1);
        let held = gate.acquire(Duration::ZERO).unwrap();
        assert_eq!(
            gate.acquire(Duration::from_millis(5)).err(),
            Some(Rejected::Timeout)
        );
        assert_eq!(gate.status().queued, 0);
        drop(held);
        assert!(gate.acquire(Duration::ZERO).is_ok());
    }

    #[test]
    fn connection_gate_rejects_without_queueing() {
        let gate = Gate::new(2, 0);
        let first = gate.acquire(Duration::ZERO).unwrap();
        let second = gate.acquire(Duration::ZERO).unwrap();
        assert_eq!(gate.acquire(Duration::ZERO).err(), Some(Rejected::Full));
        drop((first, second));
        assert_eq!(gate.status().active, 0);
    }
}
