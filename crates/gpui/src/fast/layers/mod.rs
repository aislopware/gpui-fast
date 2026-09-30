//! Scroll layers: a scroll container's content painted once into cached
//! tiles and composited at the scroll offset on frames where only the
//! offset changed. See docs/superpowers/specs/2026-09-30-scroll-layers-design.md.

pub mod scene;
