//! How many times a sprite atlas changed: a tile added, removed or every
//! tile dropped. A frame whose scene names the same tiles as the frame on
//! screen draws the same pixels only while no tile changed under them, as a
//! removed tile's place handed to another image does, so a platform says the
//! atlas changed through its presentation epoch
//! ([`crate::PlatformWindow::presentation_epoch`], [`crate::fast::same_frame`]).

use crate::AtlasState;

/// A sprite atlas's change count.
#[derive(Default)]
pub(crate) struct AtlasGeneration(u64);

impl AtlasGeneration {
    /// Counts a change.
    pub(crate) fn bump(&mut self) {
        self.0 = self.0.wrapping_add(1);
    }
}

impl<Backend> AtlasState<Backend> {
    /// How many times a tile was added or removed, or every tile dropped.
    pub fn generation(&self) -> u64 {
        self.fast_generation.0
    }
}
