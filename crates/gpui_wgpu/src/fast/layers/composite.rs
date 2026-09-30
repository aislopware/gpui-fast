//! Drawing scroll layer tiles where the scene composites them: polychrome
//! sprites whose texture id lies in the range no atlas allocates stand for
//! tiles, and are drawn from the tiles' textures.

use gpui::{LayerKey, Scene, TileCoord, decode_layer_tile};

/// The layer tiles `scene` composites, in the order it lists them.
pub(crate) fn composited_tiles(scene: &Scene) -> impl Iterator<Item = (LayerKey, TileCoord)> + '_ {
    let has_layers = !scene.layers.frames.is_empty();
    scene
        .polychrome_sprites
        .iter()
        .take_while(move |_| has_layers)
        .filter_map(|sprite| decode_layer_tile(sprite.tile.texture_id, sprite.tile.tile_id))
}
