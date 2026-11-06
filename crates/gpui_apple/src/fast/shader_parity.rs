//! Pixel tests of what the shaders draw as monocode's (onicode's base) do,
//! read back from a headless renderer: a drop shadow only outside its
//! element, and a dashed border's 3:2 rhythm (`shaders.metal`).

use std::sync::Arc;

use gpui::{
    BorderStyle, Bounds, ContentMask, Corners, DevicePixels, Edges, Hsla, Quad, ScaledPixels,
    Scene, Shadow, point, rgba, size,
};
use parking_lot::Mutex;

use crate::metal_renderer::{InstanceBufferPool, MetalRenderer};

fn sp(x: f32, y: f32, width: f32, height: f32) -> Bounds<ScaledPixels> {
    Bounds {
        origin: point(ScaledPixels(x), ScaledPixels(y)),
        size: size(ScaledPixels(width), ScaledPixels(height)),
    }
}

fn white() -> Hsla {
    Hsla::from(rgba(0xffffffff))
}

/// `draw` over black in a `width` × `height` target, as the red channel of
/// each pixel, or `None` without a Metal device.
fn render(width: i32, height: i32, draw: impl FnOnce(&mut Scene)) -> Option<Vec<u8>> {
    if objc2_metal::MTLCreateSystemDefaultDevice().is_none() {
        eprintln!("skipped: no Metal device");
        return None;
    }
    let mut renderer =
        MetalRenderer::new_headless(Arc::new(Mutex::new(InstanceBufferPool::default()))).unwrap();
    let mut scene = Scene::default();
    let everywhere = sp(0., 0., width as f32, height as f32);
    scene.insert_primitive(Quad {
        bounds: everywhere,
        content_mask: ContentMask { bounds: everywhere },
        background: Hsla::from(rgba(0x000000ff)).into(),
        ..Quad::default()
    });
    draw(&mut scene);
    scene.finish();
    let pixels = renderer
        .render_scene_to_image(&scene, size(DevicePixels(width), DevicePixels(height)))
        .expect("scene rendered")
        .into_raw();
    Some(pixels.chunks(4).map(|pixel| pixel[0]).collect())
}

/// The signed distance of `(x, y)` from the rectangle `bounds` with corners
/// of `radius`, negative inside, as `quad_sdf` in `shaders.metal` works it
/// out.
fn quad_sdf(x: f32, y: f32, bounds: Bounds<ScaledPixels>, radius: f32) -> f32 {
    let half = (bounds.size.width.0 / 2., bounds.size.height.0 / 2.);
    let center = (bounds.origin.x.0 + half.0, bounds.origin.y.0 + half.1);
    let q = (
        (x - center.0).abs() - half.0 + radius,
        (y - center.1).abs() - half.1 + radius,
    );
    if radius == 0. {
        return q.0.max(q.1);
    }
    (q.0.max(0.).powi(2) + q.1.max(0.).powi(2)).sqrt() + q.0.max(q.1).min(0.) - radius
}

/// A drop shadow is drawn only outside its element, antialiased along the
/// element's edge (`saturate(0.5 + d)`), as CSS box-shadow is: an element
/// left unfilled shows what is behind it, not its own shadow. An inset shadow
/// still draws only inside.
#[test]
fn a_drop_shadow_is_drawn_only_outside_its_element() {
    const SIDE: i32 = 64;
    const RADIUS: f32 = 6.;
    let element = sp(20., 20., 24., 24.);
    let shadow = sp(12., 14., 40., 40.);
    let primitive = |bounds, inset| Shadow {
        order: 0,
        blur_radius: ScaledPixels(0.),
        bounds,
        corner_radii: Corners::all(ScaledPixels(RADIUS)),
        content_mask: ContentMask {
            bounds: sp(0., 0., SIDE as f32, SIDE as f32),
        },
        color: white(),
        element_bounds: element,
        element_corner_radii: Corners::all(ScaledPixels(RADIUS)),
        inset,
        pad: 0,
    };
    let Some(pixels) = render(SIDE, SIDE, |scene| {
        scene.insert_primitive(primitive(shadow, 0))
    }) else {
        return;
    };
    let mut under_the_element = 0;
    for y in 0..SIDE as usize {
        for x in 0..SIDE as usize {
            let (cx, cy) = (x as f32 + 0.5, y as f32 + 0.5);
            let drawn = (0.5 - quad_sdf(cx, cy, shadow, RADIUS)).clamp(0., 1.);
            let outside = (0.5 + quad_sdf(cx, cy, element, RADIUS)).clamp(0., 1.);
            if drawn > 0. && outside < 1. {
                under_the_element += 1;
            }
            let expected = (drawn * outside * 255.).round() as i32;
            let actual = i32::from(pixels[y * SIDE as usize + x]);
            assert!(
                (actual - expected).abs() <= 2,
                "({x}, {y}): {actual}, expected {expected}"
            );
        }
    }
    assert!(under_the_element > 400, "the element lies over the shadow");
    assert_eq!(
        pixels[32 * SIDE as usize + 32],
        0,
        "nothing under the element"
    );
    assert_eq!(pixels[50 * SIDE as usize + 32], 255, "the shadow below it");

    // An inset shadow, the element less a hole, is drawn inside only.
    let hole = sp(24., 24., 16., 16.);
    let Some(pixels) = render(SIDE, SIDE, |scene| {
        scene.insert_primitive(primitive(hole, 1))
    }) else {
        return;
    };
    assert_eq!(
        pixels[22 * SIDE as usize + 32],
        255,
        "inside, outside the hole"
    );
    assert_eq!(pixels[32 * SIDE as usize + 32], 0, "in the hole");
    assert_eq!(pixels[10 * SIDE as usize + 32], 0, "outside the element");
}

/// A dashed border's dashes are three border widths long and its gaps two,
/// the rhythm WebKit draws, so three fifths of a straight side are lit.
#[test]
fn a_dashed_border_runs_three_widths_on_and_two_off() {
    const WIDTH: i32 = 240;
    const HEIGHT: i32 = 40;
    const BORDER: f32 = 2.;
    let Some(pixels) = render(WIDTH, HEIGHT, |scene| {
        let bounds = sp(0., 0., WIDTH as f32, HEIGHT as f32);
        scene.insert_primitive(Quad {
            bounds,
            content_mask: ContentMask { bounds },
            background: gpui::transparent_black().into(),
            border_color: white(),
            border_widths: Edges::all(ScaledPixels(BORDER)),
            border_style: BorderStyle::Dashed,
            ..Quad::default()
        });
    }) else {
        return;
    };
    // The top border's second row, clear of the corners.
    let row = &pixels[WIDTH as usize..2 * WIDTH as usize];
    let span = &row[20..220];
    let lit = span
        .iter()
        .map(|level| f32::from(*level) / 255.)
        .sum::<f32>()
        / span.len() as f32;
    assert!((lit - 0.6).abs() < 0.03, "{lit} of the side is lit");

    // Each dash is about six pixels and each gap about four.
    let mut runs = Vec::new();
    let mut run = (span[0] > 127, 0);
    for level in span {
        if (*level > 127) == run.0 {
            run.1 += 1;
        } else {
            runs.push(run);
            run = (*level > 127, 1);
        }
    }
    // The first and last runs are cut by the span.
    for (on, length) in &runs[1..] {
        let expected = if *on { 3. * BORDER } else { 2. * BORDER };
        assert!(
            (*length as f32 - expected).abs() <= 1.,
            "a {} of {length} pixels",
            if *on { "dash" } else { "gap" }
        );
    }
}
