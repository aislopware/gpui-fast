//! Drawing scroll layer tiles where the scene composites them: polychrome
//! sprites whose texture id lies in the range no atlas allocates stand for
//! tiles, and are drawn with the polychrome sprite pipeline from the tiles'
//! own textures instead of an atlas texture.
//!
//! A tile sprite covers the tile's whole texture at whole device pixels, so
//! every fragment samples a texel center and copies the tile's pixel.

use std::ops::Range;

use gpui::{AtlasTextureId, DevicePixels, Size, TileId, decode_layer_tile};

use crate::metal_renderer::{
    InstanceBindings, MetalRenderer,
    binds::{Binds, Instanced},
};

/// Forwarded to by both ways a frame's polychrome sprites are drawn: the
/// fork's bound draws (`MetalRenderer::draw_bound`) and upstream's
/// `MetalRenderer::draw_polychrome_sprites`. Draws the batch `sprites` if its
/// texture is a layer's tiles, and returns whether it was: otherwise the
/// caller draws it from the atlas. A tile the cache does not hold draws
/// nothing; rasterizing every composited tile it lacks before the frame is
/// drawn prevents that.
pub(crate) fn draw_tiles(
    renderer: &MetalRenderer,
    texture_id: AtlasTextureId,
    sprites: &Range<usize>,
    instance_bindings: &InstanceBindings,
    viewport_size: Size<DevicePixels>,
    command_encoder: &metal::RenderCommandEncoderRef,
    binds: &mut Binds,
) -> bool {
    let Some((layer, _)) = decode_layer_tile(texture_id, TileId(0)) else {
        return false;
    };
    let cache = &renderer.fast_layers;
    for (tile, run) in cache.tile_runs(sprites.clone()) {
        let Some(texture) = cache.texture(layer, tile) else {
            continue;
        };
        renderer.draw_instanced(
            Instanced {
                pipeline: &renderer.polychrome_sprites_pipeline_state,
                instances: &instance_bindings.polychrome_sprites,
                fragment_reads_instances: true,
                atlas: Some(texture),
                range: run,
            },
            viewport_size,
            command_encoder,
            binds,
        );
    }
    true
}
