use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

#[derive(Debug)]
pub(crate) struct RestartGuard {
    window: Duration,
    max_restarts: usize,
    restarts: VecDeque<Instant>,
}

impl RestartGuard {
    pub(crate) fn new(window: Duration, max_restarts: usize) -> Self {
        Self {
            window,
            max_restarts,
            restarts: VecDeque::new(),
        }
    }

    pub(crate) fn allow_restart(&mut self, now: Instant) -> bool {
        while let Some(oldest) = self.restarts.front() {
            if now.duration_since(*oldest) > self.window {
                self.restarts.pop_front();
            } else {
                break;
            }
        }

        if self.restarts.len() >= self.max_restarts {
            return false;
        }
        self.restarts.push_back(now);
        true
    }

    pub(crate) fn restart_attempts_in_window(&self) -> usize {
        self.restarts.len()
    }

    pub(crate) fn max_restarts_in_window(&self) -> usize {
        self.max_restarts
    }
}
