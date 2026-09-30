//! Where the headless atlas puts a tile.

use crate::AtlasTextureKind;

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
