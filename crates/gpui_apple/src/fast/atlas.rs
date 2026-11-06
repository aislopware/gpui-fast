//! Tiles bigger than the sprite atlas can ever hold.
//!
//! Upstream's `MetalAtlasTextures::allocate` adds a texture when none it has
//! has room for a tile, sized to the tile but clamped to 16384 on a side, the
//! largest texture Apple GPUs make. A tile wider or taller than that does not
//! fit the new texture either: its allocation fails, the empty texture stays
//! in the atlas, nothing is cached, and the next frame that paints the image
//! adds another. An image of 32768 × 16384 thus adds a 16384² BGRA texture
//! (1 GiB) on every frame until the process runs out of memory. `allocate`
//! asks [`ensure_fits`] first and fails without adding a texture.
//!
//! longbridge/gpui-fast#103 fixed this in the wgpu atlas; this is the same fix
//! for Metal's, which longbridge's change does not reach.

use anyhow::{Result, ensure};
use gpui::{DevicePixels, Size};

/// The largest texture side the atlas makes: `push_texture`'s clamp.
pub(crate) const MAX_TEXTURE_SIDE: i32 = 16384;

/// Whether a tile of `size` can fit in a texture no bigger than `max_side` on
/// each side.
pub(crate) fn fits(size: Size<DevicePixels>, max_side: i32) -> bool {
    size.width.0 <= max_side && size.height.0 <= max_side
}

/// Fails for a tile no atlas texture can hold, before one is added for it.
pub(crate) fn ensure_fits(size: Size<DevicePixels>) -> Result<()> {
    ensure!(
        fits(size, MAX_TEXTURE_SIDE),
        "a {}×{} atlas tile is larger than the largest atlas texture ({MAX_TEXTURE_SIDE} a side)",
        size.width.0,
        size.height.0
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{MAX_TEXTURE_SIDE, ensure_fits, fits};
    use gpui::{DevicePixels, Size};

    fn tile(width: i32, height: i32) -> Size<DevicePixels> {
        gpui::size(DevicePixels(width), DevicePixels(height))
    }

    #[test]
    fn a_tile_over_the_limit_on_either_side_never_fits() {
        assert!(!fits(tile(32768, 16384), MAX_TEXTURE_SIDE));
        assert!(!fits(tile(16385, 1), MAX_TEXTURE_SIDE));
        assert!(!fits(tile(1, 16385), MAX_TEXTURE_SIDE));
        assert!(ensure_fits(tile(32768, 16384)).is_err());
    }

    #[test]
    fn a_tile_up_to_the_limit_fits() {
        assert!(fits(tile(16384, 16384), MAX_TEXTURE_SIDE));
        assert!(fits(tile(1920, 1080), MAX_TEXTURE_SIDE));
        assert!(ensure_fits(tile(1, 1)).is_ok());
    }

    /// The atlas refuses a tile wider than its largest texture without adding a
    /// texture for it, so painting it on every frame holds no memory: three
    /// refusals later, a small tile still opens the atlas's first texture.
    /// Before, each refusal left a 16384 × 1024 texture (64 MiB) behind and the
    /// small tile went into the third.
    #[test]
    fn an_oversized_tile_adds_no_texture() {
        use gpui::{AtlasKey, PlatformAtlas as _};
        use objc2_metal::{MTLDevice as _, MTLGPUFamily};
        use std::borrow::Cow;

        let Some(device) = objc2_metal::MTLCreateSystemDefaultDevice() else {
            return;
        };
        let shared = cfg!(target_os = "ios") || device.supportsFamily(MTLGPUFamily::Apple1);
        let atlas = crate::metal_atlas::MetalAtlas::new(device, shared);
        let image = |id: usize| {
            AtlasKey::Image(gpui::RenderImageParams {
                image_id: gpui::ImageId(id),
                frame_index: 0,
            })
        };
        let insert = |key: AtlasKey, size: Size<DevicePixels>| {
            let bytes = vec![0u8; size.width.0 as usize * size.height.0 as usize * 4];
            atlas.get_or_insert_with(key, &mut || Ok(Some((size, Cow::Owned(bytes.clone())))))
        };
        for id in 0..3 {
            assert!(insert(image(id), tile(MAX_TEXTURE_SIDE + 1, 1)).is_err());
        }
        let small = insert(image(3), tile(64, 64))
            .expect("inserting a small tile")
            .expect("a small tile is kept");
        assert_eq!(small.texture_id.index, 0);
    }

    /// Clearing the atlas (`PlatformAtlas::clear`, gpui's `fast::atlas_clear`)
    /// drops its textures: a tile of before names a texture no longer there,
    /// and a key inserted again builds its tile again, in a texture of its own.
    #[test]
    fn a_cleared_atlas_holds_no_texture_and_builds_again() {
        use gpui::{AtlasKey, PlatformAtlas as _};
        use objc2_metal::{MTLDevice as _, MTLGPUFamily};
        use std::borrow::Cow;

        let Some(device) = objc2_metal::MTLCreateSystemDefaultDevice() else {
            return;
        };
        let shared = cfg!(target_os = "ios") || device.supportsFamily(MTLGPUFamily::Apple1);
        let atlas = crate::metal_atlas::MetalAtlas::new(device, shared);
        let key = |id: usize| {
            AtlasKey::Image(gpui::RenderImageParams {
                image_id: gpui::ImageId(id),
                frame_index: 0,
            })
        };
        let builds = std::cell::Cell::new(0);
        let insert = |key: AtlasKey| {
            let size = tile(64, 64);
            let bytes = vec![0u8; 64 * 64 * 4];
            atlas
                .get_or_insert_with(key, &mut || {
                    builds.set(builds.get() + 1);
                    Ok(Some((size, Cow::Owned(bytes.clone()))))
                })
                .expect("inserting a tile")
                .expect("the tile is kept")
        };
        let tiles: Vec<_> = (0..3).map(|id| insert(key(id))).collect();
        assert_eq!(builds.get(), 3);
        insert(key(0));
        assert_eq!(builds.get(), 3, "a kept tile is not built again");
        assert!(atlas.metal_texture(tiles[0].texture_id).is_some());
        assert_eq!(atlas.fast_textures().len(), 1);

        atlas.clear();
        assert!(atlas.fast_textures().is_empty(), "no texture is held");
        assert!(atlas.metal_texture(tiles[0].texture_id).is_none());

        let again = insert(key(0));
        assert_eq!(builds.get(), 4, "a key inserted again is built again");
        assert!(atlas.metal_texture(again.texture_id).is_some());
        assert_eq!(atlas.fast_textures().len(), 1);
    }
}
