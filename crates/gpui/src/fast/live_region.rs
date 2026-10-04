//! An element as a live region: a change to its value is announced without
//! the element taking focus, as a count that settles or a status that changes.
//!
//! Upstream's elements set every accessibility property AccessKit has but
//! `live`, so nothing an application draws can ask to be announced. Here
//! [`LiveRegion::aria_live`] sets it on the element's node. When the node's
//! value changes, or the node first appears with one, AccessKit's adapters
//! announce the value. On macOS that is
//! `NSAccessibilityAnnouncementRequestedNotification`, at medium priority for
//! [`accesskit::Live::Polite`] and high for [`accesskit::Live::Assertive`].
//!
//! Announce a settled value: a value that changes on every frame is announced
//! on every frame, so debounce a running count before setting it.

use crate::StatefulInteractiveElement;

/// An element that can be a live region. Like the other accessibility
/// properties, it reaches the tree only on an element with an id and a role.
pub trait LiveRegion: StatefulInteractiveElement {
    /// Announce changes to this element's value: [`accesskit::Live::Polite`]
    /// after what is being said, [`accesskit::Live::Assertive`] at once.
    fn aria_live(mut self, live: accesskit::Live) -> Self {
        crate::fast::interactivity::Aria::set_live(&mut self.interactivity().aria, live);
        self
    }
}

impl<E: StatefulInteractiveElement> LiveRegion for E {}
