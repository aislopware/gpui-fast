//! Emptying every window's sprite atlas, for an application under memory
//! pressure (monocode's `App::clear_sprite_atlases`; FORK.md, "Onicode").
//!
//! A sprite atlas keeps every glyph, icon and image tile a window ever drew
//! until its key is removed, which only an image's eviction does; a long
//! session holds megabytes of tiles for text long gone from the screen.
//! [`App::clear_sprite_atlases`] drops them all
//! ([`crate::PlatformAtlas::clear`]), and the frames after upload only what
//! they draw again.
//!
//! What still names a tile of the old atlas is what draws a frame from the
//! last one: retained views, keyed paint and spliced stretches replayed from
//! the rendered scene, the glyph tiles [`crate::fast::glyphs`] keeps for the
//! frame, and scroll layers, whose rows keep their scenes to extend their
//! tiles with. A cleared atlas hands its texture slots out again, so a
//! replayed sprite would sample whatever the new texture holds at its old
//! place. So each cleared window drops its layers, forgets its glyph tiles,
//! and is refreshed: its next frame draws everything afresh, as after a GPU
//! device is lost.

use crate::{App, Window};

impl App {
    /// Empties every window's sprite atlas and redraws them all, so glyphs,
    /// icons and images come back only for what is on screen. Atlases
    /// otherwise keep everything ever drawn. A window mid-update is skipped,
    /// so call this from a task rather than inside a window's update.
    ///
    /// Only Metal's atlas (macOS and iOS) and the headless one empty; the
    /// others keep their tiles, and their windows are just drawn again.
    pub fn clear_sprite_atlases(&mut self) {
        for window in self.windows.values_mut().flatten() {
            clear(window);
        }
        self.refresh_windows();
    }
}

/// Empties `window`'s sprite atlas and lets nothing of its last frame be
/// drawn again.
fn clear(window: &mut Window) {
    window.sprite_atlas.clear();
    crate::fast::glyphs::GlyphBoundsCache::forget_tiles(&mut window.fast_glyph_bounds);
    window.fast_layers.layers.clear();
    window.refresh();
}
