//! Tests of scroll layers.

use crate::{
    AtlasTextureId, AtlasTextureKind, LayerKey, TileCoord, decode_layer_tile, layer_tile_id,
    layer_tile_texture_id,
};

#[test]
fn layer_tile_ids_round_trip_and_never_collide_with_the_atlas() {
    for layer in [0, 1, 77, 0x00FF_FFFF] {
        for tile in [(0, 0), (-1, 3), (2047, -2048), (-2048, 2047)] {
            let key = LayerKey(layer);
            let coord = TileCoord {
                x: tile.0,
                y: tile.1,
            };
            let texture = layer_tile_texture_id(key);
            assert_eq!(texture.kind, AtlasTextureKind::Polychrome);
            assert!(texture.index >= crate::LAYER_TILE_TEXTURE_BASE);
            assert_eq!(
                decode_layer_tile(texture, layer_tile_id(coord)),
                Some((key, coord))
            );
        }
    }
    let atlas = AtlasTextureId {
        index: 3,
        kind: AtlasTextureKind::Polychrome,
    };
    assert_eq!(
        decode_layer_tile(atlas, layer_tile_id(TileCoord { x: 0, y: 0 })),
        None
    );
    let mono = AtlasTextureId {
        index: crate::LAYER_TILE_TEXTURE_BASE,
        kind: AtlasTextureKind::Monochrome,
    };
    assert_eq!(
        decode_layer_tile(mono, layer_tile_id(TileCoord { x: 0, y: 0 })),
        None
    );
}
