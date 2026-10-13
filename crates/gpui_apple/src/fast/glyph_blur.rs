//! Pixel tests of blurred and transformed glyphs (gpui's
//! `fast::glyph_blur`; the shaders' `glyph_blur_*`): a headless renderer
//! draws a solid glyph tile, read back against the kernel worked out here.

use std::{borrow::Cow, sync::Arc};

use gpui::{
    AtlasKey, Bounds, ContentMask, DevicePixels, Hsla, MonochromeSprite, PlatformAtlas as _, Quad,
    RenderSvgParams, ScaledPixels, Scene, TransformationMatrix, point, rgba, size,
};
use parking_lot::Mutex;

use crate::metal_renderer::{InstanceBufferPool, MetalRenderer};

const SIDE: i32 = 96;
const GLYPH: i32 = 16;

fn sp(x: f32, y: f32, width: f32, height: f32) -> Bounds<ScaledPixels> {
    Bounds {
        origin: point(ScaledPixels(x), ScaledPixels(y)),
        size: size(ScaledPixels(width), ScaledPixels(height)),
    }
}

/// Draws a solid white `GLYPH`-pixel square glyph at (40, 40) over black,
/// blurred by `sigma` and through `transformation`, and returns the red
/// channel.
fn draw(sigma: f32, transformation: TransformationMatrix) -> Option<Vec<u8>> {
    if metal::Device::system_default().is_none() {
        eprintln!("skipped: no Metal device");
        return None;
    }
    let mut renderer =
        MetalRenderer::new_headless(Arc::new(Mutex::new(InstanceBufferPool::default())));
    let mut tile = renderer
        .sprite_atlas()
        .get_or_insert_with(
            AtlasKey::Svg(RenderSvgParams {
                path: "a solid square glyph".into(),
                size: size(DevicePixels(GLYPH), DevicePixels(GLYPH)),
            }),
            &mut || {
                Ok(Some((
                    size(DevicePixels(GLYPH), DevicePixels(GLYPH)),
                    Cow::Owned(vec![255; (GLYPH * GLYPH) as usize]),
                )))
            },
        )
        .expect("glyph uploaded")
        .expect("glyph tile");
    tile.padding = sigma.to_bits();
    let everywhere = ContentMask {
        bounds: sp(0., 0., SIDE as f32, SIDE as f32),
    };
    let mut scene = Scene::default();
    scene.insert_primitive(Quad {
        bounds: everywhere.bounds,
        content_mask: everywhere,
        background: Hsla::from(rgba(0x000000ff)).into(),
        ..Default::default()
    });
    scene.insert_primitive(MonochromeSprite {
        order: 0,
        pad: 0,
        bounds: sp(40., 40., GLYPH as f32, GLYPH as f32),
        content_mask: everywhere,
        color: Hsla::from(rgba(0xffffffff)),
        tile,
        transformation,
    });
    scene.finish();
    let image = renderer
        .render_scene_to_image(&scene, size(DevicePixels(SIDE), DevicePixels(SIDE)))
        .expect("scene rendered");
    Some(image.pixels().map(|pixel| pixel[0]).collect())
}

/// The glyph's coverage at the pixel centred at (`x`, `y`) under zui's
/// kernel: gaussian taps a texel apart out to three standard deviations
/// (twelve at most), taps off the tile counting as transparent.
fn expected(sigma: f32, x: i32, y: i32) -> f32 {
    let radius = ((3. * sigma).ceil() as i32).min(12);
    let (mut accum, mut total) = (0., 0.);
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            let weight = (-((dx * dx + dy * dy) as f32) / (2. * sigma * sigma)).exp();
            let inside =
                (40..40 + GLYPH).contains(&(x + dx)) && (40..40 + GLYPH).contains(&(y + dy));
            accum += if inside { weight } else { 0. };
            total += weight;
        }
    }
    accum / total
}

/// A blurred glyph is the kernel's convolution of its tile, halo and all,
/// to within the rounding of 8 bits; a blur of 0 draws it sharp.
#[test]
fn a_blurred_glyph_draws_its_gaussian() {
    for sigma in [1.5, 3.] {
        let Some(pixels) = draw(sigma, TransformationMatrix::unit()) else {
            return;
        };
        let mut worst = 0;
        for y in 0..SIDE {
            for x in 0..SIDE {
                let expected = (expected(sigma, x, y) * 255.).round() as i32;
                let actual = i32::from(pixels[(y * SIDE + x) as usize]);
                worst = worst.max((actual - expected).abs());
            }
        }
        eprintln!("glyph under σ {sigma}: within {worst} of 255 levels");
        assert!(worst <= 2, "σ {sigma}: {worst}");
        // The halo reaches past the glyph's bounds.
        assert!(pixels[(48 * SIDE + 37) as usize] > 0);
    }
    let sharp = draw(0., TransformationMatrix::unit()).unwrap();
    assert_eq!(sharp[(48 * SIDE + 39) as usize], 0);
    assert_eq!(sharp[(48 * SIDE + 40) as usize], 255);
}

/// The transformation scales the drawn glyph, its halo with it, about the
/// point it is built around, without another raster.
#[test]
fn a_transformed_blurred_glyph_scales_with_its_halo() {
    let pivot = point(ScaledPixels(48.), ScaledPixels(48.));
    let matrix = TransformationMatrix::unit()
        .translate(pivot)
        .scale(size(2., 2.))
        .translate(point(ScaledPixels(-48.), ScaledPixels(-48.)));
    let Some(scaled) = draw(1.5, matrix) else {
        return;
    };
    // The square spans 32..64 now; its centre stays white, past 64 the halo
    // fades over twice the reach.
    assert_eq!(scaled[(48 * SIDE + 48) as usize], 255);
    assert!(scaled[(48 * SIDE + 66) as usize] > 0);
    assert!(scaled[(48 * SIDE + 66) as usize] < 128);
    assert!(
        scaled[(48 * SIDE + 30) as usize] > 0,
        "the halo before the square"
    );
}
