//! Momentum: the scrolling that carries on after the fingers lift, told apart
//! from the scrolling a finger or a wheel drives
//! ([`ScrollWheelEvent::momentum_phase`]).

use crate::{ScrollWheelEvent, TouchPhase};

impl ScrollWheelEvent {
    /// This scroll step, as a step of momentum in `phase`.
    pub(crate) fn fast_momentum(mut self, phase: TouchPhase) -> Self {
        self.momentum_phase = Some(phase);
        self
    }
}
