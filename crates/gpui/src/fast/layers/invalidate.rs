//! Telling scrolls and offset reads apart from other changes, and deciding
//! whether a frame only scrolled a layer (M4).

#[derive(Default)]
pub(crate) struct ScrollLog {}
