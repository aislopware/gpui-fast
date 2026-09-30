//! How many subscribers a [`SubscriberSet`] holds, for
//! [`crate::fast::subscriptions`]. A child of `subscription.rs`, whose fields
//! it reads.

use super::SubscriberSet;

impl<EmitterKey, Callback> SubscriberSet<EmitterKey, Callback> {
    /// The subscribers registered, active or not yet activated. Those of an
    /// emitter whose callbacks are running at the time are taken out and not
    /// counted.
    pub(crate) fn len(&self) -> usize {
        self.0
            .borrow()
            .subscribers
            .values()
            .map(|subscribers| subscribers.as_ref().map_or(0, |subscribers| subscribers.len()))
            .sum()
    }
}
