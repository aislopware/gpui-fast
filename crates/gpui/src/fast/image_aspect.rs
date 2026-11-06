//! When `img` applies an image's intrinsic aspect ratio.
//!
//! `img` stamped the image's ratio into its style whenever the style named
//! none, even when it gave both a width and a height. Layout then sized the
//! element past those dimensions: a 100 × 400 image styled
//! `w_full().h(px(60.))` in a 100 px column laid out at 100 × 400 and
//! overflowed its clip, so rounded corners landed off screen. CSS applies a
//! preferred aspect ratio only when at least one dimension is auto, and so
//! does `img` now.
//!
//! Ported from zui aeef0674e (wingleeio/zed), Apache-2.0.

use crate::{Length, Style};

/// Whether an image's intrinsic aspect ratio may size an element of this
/// style: the style names no ratio of its own and leaves a dimension auto.
pub(crate) fn applies_intrinsic_ratio(style: &Style) -> bool {
    style.aspect_ratio.is_none()
        && (style.size.width == Length::Auto || style.size.height == Length::Auto)
}
