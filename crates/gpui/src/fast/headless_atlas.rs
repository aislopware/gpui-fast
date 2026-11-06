//! Where the headless atlas puts a tile, and its emptying.

use crate::{AtlasState, AtlasTextureKind, HeadlessAtlasBackend};

/// The texture a headless atlas puts every tile of `kind` in: one per kind,
/// as a GPU atlas holds a window's glyphs in one texture until it fills.
///
/// Upstream gave every tile a texture of its own, so a frame drawn headless
/// broke its sprites into a batch per glyph and sorted them by thousands of
/// textures, and a glyph rasterized for the first time put every sprite's
/// key after it in a new place: nothing a window on a GPU does.
pub(crate) fn texture(kind: AtlasTextureKind) -> u32 {
    kind as u32
}

/// Empties a headless atlas (`PlatformAtlas::clear`, `fast::atlas_clear`).
/// Its tile ids keep counting up, so a tile inserted after is told from one
/// inserted before, as a GPU atlas's new tile lies elsewhere.
pub(crate) fn clear(state: &mut AtlasState<HeadlessAtlasBackend>) {
    state.clear(|_| {});
}
