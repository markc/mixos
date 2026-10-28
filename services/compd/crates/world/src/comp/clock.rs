// SPDX-License-Identifier: MIT OR Apache-2.0
//! Policy time. Production reads the monotonic clock; fixtures own frozen time.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

#[derive(Clone, Default)]
pub enum Clock {
    #[default]
    Monotonic,
    Manual(ManualClock),
}

impl Clock {
    pub fn now(&self) -> Instant {
        match self {
            Self::Monotonic => Instant::now(),
            Self::Manual(clock) => clock.now(),
        }
    }
}

#[derive(Clone)]
pub struct ManualClock(Rc<Cell<Instant>>);

impl Default for ManualClock {
    fn default() -> Self {
        Self(Rc::new(Cell::new(Instant::now())))
    }
}

impl ManualClock {
    pub fn now(&self) -> Instant {
        self.0.get()
    }

    pub fn advance(&self, duration: Duration) {
        self.0.set(self.now() + duration);
    }
}
