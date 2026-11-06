//! How many drawables a window's `CAMetalLayer` keeps (`GPUI_DRAWABLE_COUNT`).
//!
//! Three, as upstream, unless `GPUI_DRAWABLE_COUNT` says 2. Two keeps one
//! drawable on the glass and one for the next frame; each drawable is a
//! full-resolution surface (a full-screen window on a 5K display takes
//! 5120 × 2880 × 4 bytes, 56 MiB) that every window holds while idle. Three
//! lets a frame be encoded while one is on the glass and another is queued,
//! which pacing at a high refresh rate leans on: with two, `nextDrawable`
//! blocks sooner, and a window found blocking moves to the
//! `CAMetalDisplayLink` (`fast::frame_source`).
//!
//! The default stays three until a measurement says otherwise: physical
//! footprint and `GPUI_GPU_STATS=1`'s device total (`fast::gpu_stats`) across
//! idle and animating windows, against the frames presentation reports show
//! dropped or late.
//!
//! Ported from zui 36ca993fb (wing-anara), Apache-2.0, as an option rather
//! than a new default.

use std::sync::OnceLock;

/// The drawables a window keeps unless asked otherwise.
const DEFAULT: usize = 3;

/// How many drawables a window's layer keeps: `GPUI_DRAWABLE_COUNT`, read once.
pub(crate) fn drawable_count() -> usize {
    static COUNT: OnceLock<usize> = OnceLock::new();
    *COUNT.get_or_init(|| parse(std::env::var("GPUI_DRAWABLE_COUNT").ok().as_deref()))
}

/// The count `GPUI_DRAWABLE_COUNT` asks for. Core Animation takes 2 or 3;
/// anything else keeps the default, with a warning.
fn parse(value: Option<&str>) -> usize {
    match value.map(str::trim) {
        None | Some("") => DEFAULT,
        Some("2") => 2,
        Some("3") => 3,
        Some(other) => {
            log::warn!("GPUI_DRAWABLE_COUNT={other:?} is not 2 or 3; keeping {DEFAULT}");
            DEFAULT
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use parking_lot::Mutex;

    use super::{DEFAULT, drawable_count, parse};
    use crate::metal_renderer::{InstanceBufferPool, MetalRenderer};

    #[test]
    fn the_variable_asks_for_two_or_three() {
        assert_eq!(parse(None), DEFAULT);
        assert_eq!(parse(Some("")), DEFAULT);
        assert_eq!(parse(Some("2")), 2);
        assert_eq!(parse(Some(" 2\n")), 2);
        assert_eq!(parse(Some("3")), 3);
        for refused in ["1", "4", "0", "two", "-2", "2.0"] {
            assert_eq!(parse(Some(refused)), DEFAULT, "{refused:?}");
        }
        assert_eq!(DEFAULT, 3, "upstream's count until a measurement moves it");
    }

    #[test]
    fn a_window_layer_keeps_the_drawable_count() {
        if objc2_metal::MTLCreateSystemDefaultDevice().is_none() {
            eprintln!("skipped: no Metal device");
            return;
        }
        let renderer = MetalRenderer::new(
            Arc::new(Mutex::new(InstanceBufferPool::default())),
            false,
            false,
        )
        .unwrap();
        let layer = renderer.layer().expect("a window renderer has a layer");
        assert_eq!(layer.maximumDrawableCount(), drawable_count());
    }
}
