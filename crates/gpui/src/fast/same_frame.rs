//! A frame that draws what the screen already shows is not presented.
//!
//! A window draws when something told it to (a notify, a hover, a timer), and
//! what it draws then is often what it drew before: a clock that renders the
//! same minute every second, a view notified for state it does not show, a
//! hover over something with no hover style. Each of those frames used to
//! take a drawable, encode the whole scene, present it and wake the display
//! for a frame no one could tell from the last. So a frame drawn while the
//! frame before it is what the screen shows, and whose scene equals that
//! frame's, is not handed to the platform: no drawable is taken and nothing
//! is presented.
//!
//! A frame is left unpresented only when all of these hold:
//!
//! - It was drawn on a platform frame ([`crate::fast::settle`]) that neither
//!   required a presentation, forced a render (a GPU device's recovery), nor
//!   came during high-rate input, which presents to keep the display from
//!   clocking down. A present asked for without a draw always presents.
//! - No animation asked for the next frame while it was drawn: a frame in
//!   the middle of motion is not compared, as it differs from the last one.
//! - Its scene equals the one before it, which was presented or equal to one
//!   that was ([`same_scene`]): every primitive the renderer reads, the edge
//!   fades, holes, blurs and masks, and each scroll layer's generation. A
//!   scene holding a surface never counts as the same, since a video frame
//!   changes its pixels behind the same buffer.
//! - The platform says nothing it draws with changed since that present
//!   ([`crate::PlatformWindow::presentation_epoch`]: on Metal, the atlas's
//!   generation, the drawable's size and the layer's opacity), and the
//!   viewport, scale and content zoom are those it was presented at. A
//!   platform that can't say presents every frame.
//! - The window composes no natives: their placement, hit map and focus go to
//!   the platform with each present.
//!
//! `GPUI_SAME_FRAME=0` presents every frame drawn, to measure against
//! (`gpui_perf`'s `idle_presents` example).
//!
//! Frame callbacks, `on_next_frame` and animations run on the platform's
//! frame requests, not on presents, so they run as before; `Window::on_present`
//! callbacks still run for the frame, as it is what the screen shows.
//!
//! The idea is rune's (anomalyco/rune, `crates/rune/src/window/presented.rs`,
//! at `6a6c8302` and `a777e03d`; MIT): an idle window builds a frame before
//! taking a drawable, and when it would draw the same pixels, nothing is
//! acquired or presented. Rune measured one settings window go from 77
//! presents per 20 s to none. Here (`idle_presents`, Mac Studio, 2026-11-07):
//! a clock notified 10 times a second that changes once a second presents
//! 1.0 frames a second instead of 9.7, and its process takes 13.5 ms of CPU a
//! second instead of 25; notified 30 times a second for nothing, 19 instead
//! of 57.

use std::{mem, sync::OnceLock};

use crate::{
    MonochromeSprite, Path, PathVertex, Pixels, PolychromeSprite, Quad, ScaledPixels, Scene,
    Shadow, Size, SubpixelSprite, Underline, Window,
};

/// A window's account of what the screen shows.
#[derive(Default)]
pub(crate) struct SameFrame {
    /// The frame the window drew last is what the screen shows: it was
    /// presented, or draws what the frame presented drew.
    shown: bool,
    /// What the frame shown was presented into.
    presented: Option<Target>,
    /// The present under way follows a draw on a platform frame that asked
    /// for no presentation of its own.
    may_skip: bool,
    /// Frames presented, and frames left unpresented as the same.
    #[cfg(any(test, feature = "test-support"))]
    counts: PresentCounts,
}

/// What a frame is presented into, besides its scene.
#[derive(Clone, Copy, PartialEq)]
struct Target {
    epoch: u64,
    viewport: Size<Pixels>,
    scale: f32,
    zoom: f32,
}

/// How many frames a window presented and how many it left unpresented
/// because they drew what the screen showed ([`Window::present_counts`]).
#[cfg(any(test, feature = "test-support"))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PresentCounts {
    /// Frames handed to the platform.
    pub presented: u64,
    /// Frames not handed to the platform, as the same as the one shown.
    pub skipped: u64,
}

#[cfg(any(test, feature = "test-support"))]
impl Window {
    /// How many frames this window presented and how many it left
    /// unpresented because they drew what the screen showed.
    pub fn present_counts(&self) -> PresentCounts {
        self.fast_same_frame.counts
    }
}

/// Forwarded to by `Window::draw` once the frame it drew is
/// `rendered_frame` and the one before it `next_frame`: whether the screen
/// still shows what the window drew.
pub(crate) fn drawn(window: &mut Window) {
    let animating = !window.next_frame_callbacks.borrow().is_empty();
    let shown = window.fast_same_frame.shown;
    let same = !animating
        && (shown || crate::fast::churn::enabled(window))
        && same_scene(&window.rendered_frame.scene, &window.next_frame.scene);
    window.fast_same_frame.shown = shown && same;
    crate::fast::churn::frame_drawn(window, same);
}

/// Forwarded to by `fast::settle` before it presents a frame it drew:
/// `forced` when the platform frame required a presentation, forced a
/// render or came during high-rate input.
pub(crate) fn before_present(window: &mut Window, forced: bool) {
    window.fast_same_frame.may_skip = !forced;
}

/// Whether the frame being presented draws what the screen shows, so the
/// platform is not handed it. A frame that is handed over becomes the one
/// shown. `composes`: the window presents natives with its frames.
pub(crate) fn unchanged(window: &mut Window, composes: bool) -> bool {
    let target = window
        .platform_window
        .presentation_epoch()
        .map(|epoch| Target {
            epoch,
            viewport: window.viewport_size,
            scale: window.scale_factor(),
            zoom: window.content_zoom(),
        });
    let state = &mut window.fast_same_frame;
    let may_skip = mem::take(&mut state.may_skip) && enabled();
    let skip =
        may_skip && state.shown && !composes && target.is_some() && state.presented == target;
    if skip {
        #[cfg(any(test, feature = "test-support"))]
        {
            state.counts.skipped += 1;
        }
    } else {
        state.presented = target;
        state.shown = true;
        #[cfg(any(test, feature = "test-support"))]
        {
            state.counts.presented += 1;
        }
    }
    skip
}

/// Whether frames that draw what the screen shows go unpresented: unless
/// `GPUI_SAME_FRAME=0`.
fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("GPUI_SAME_FRAME").is_none_or(|value| value != "0"))
}

/// Whether `a` and `b` draw the same pixels into the same target, as far as
/// their contents tell: every field a renderer reads is compared, and a
/// field added to [`Scene`] fails to compile here until it is.
pub(crate) fn same_scene(a: &Scene, b: &Scene) -> bool {
    let Scene {
        // How the primitives were painted, and what replays them: none of it
        // is drawn.
        paint_operations: _,
        primitive_bounds: _,
        layer_stack: _,
        sort_scratch: _,
        shadows,
        quads,
        paths,
        underlines,
        monochrome_sprites,
        subpixel_sprites,
        polychrome_sprites,
        surfaces,
        composition,
        fast_painted: _,
        layers,
    } = a;
    surfaces.is_empty()
        && b.surfaces.is_empty()
        && same_all(quads, &b.quads, same_quad)
        && same_all(shadows, &b.shadows, same_shadow)
        && same_all(underlines, &b.underlines, same_underline)
        && same_all(monochrome_sprites, &b.monochrome_sprites, same_monochrome)
        && same_all(subpixel_sprites, &b.subpixel_sprites, same_subpixel)
        && same_all(polychrome_sprites, &b.polychrome_sprites, same_polychrome)
        && same_all(paths, &b.paths, same_path)
        && a.edge_fades() == b.edge_fades()
        && same_composition(composition, &b.composition)
        && same_layers(layers, &b.layers)
}

fn same_all<T>(a: &[T], b: &[T], same: fn(&T, &T) -> bool) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same(a, b))
}

fn same_quad(a: &Quad, b: &Quad) -> bool {
    let Quad {
        order,
        border_style,
        bounds,
        content_mask,
        background,
        border_color,
        corner_radii,
        border_widths,
    } = a;
    *order == b.order
        && *border_style == b.border_style
        && *bounds == b.bounds
        && *content_mask == b.content_mask
        && *background == b.background
        && *border_color == b.border_color
        && *corner_radii == b.corner_radii
        && *border_widths == b.border_widths
}

fn same_shadow(a: &Shadow, b: &Shadow) -> bool {
    let Shadow {
        order,
        blur_radius,
        bounds,
        corner_radii,
        content_mask,
        color,
        element_bounds,
        element_corner_radii,
        inset,
        pad,
    } = a;
    *order == b.order
        && *blur_radius == b.blur_radius
        && *bounds == b.bounds
        && *corner_radii == b.corner_radii
        && *content_mask == b.content_mask
        && *color == b.color
        && *element_bounds == b.element_bounds
        && *element_corner_radii == b.element_corner_radii
        && *inset == b.inset
        && *pad == b.pad
}

fn same_underline(a: &Underline, b: &Underline) -> bool {
    let Underline {
        order,
        pad,
        bounds,
        content_mask,
        color,
        thickness,
        wavy,
    } = a;
    *order == b.order
        && *pad == b.pad
        && *bounds == b.bounds
        && *content_mask == b.content_mask
        && *color == b.color
        && *thickness == b.thickness
        && *wavy == b.wavy
}

fn same_monochrome(a: &MonochromeSprite, b: &MonochromeSprite) -> bool {
    let MonochromeSprite {
        order,
        pad,
        tile,
        bounds,
        content_mask,
        color,
        transformation,
    } = a;
    *order == b.order
        && *pad == b.pad
        && *tile == b.tile
        && *bounds == b.bounds
        && *content_mask == b.content_mask
        && *color == b.color
        && *transformation == b.transformation
}

fn same_subpixel(a: &SubpixelSprite, b: &SubpixelSprite) -> bool {
    let SubpixelSprite {
        order,
        pad,
        tile,
        bounds,
        content_mask,
        color,
        transformation,
    } = a;
    *order == b.order
        && *pad == b.pad
        && *tile == b.tile
        && *bounds == b.bounds
        && *content_mask == b.content_mask
        && *color == b.color
        && *transformation == b.transformation
}

fn same_polychrome(a: &PolychromeSprite, b: &PolychromeSprite) -> bool {
    let PolychromeSprite {
        order,
        pad,
        tile,
        grayscale,
        opacity,
        bounds,
        content_mask,
        corner_radii,
    } = a;
    *order == b.order
        && *pad == b.pad
        && *tile == b.tile
        && *grayscale == b.grayscale
        && *opacity == b.opacity
        && *bounds == b.bounds
        && *content_mask == b.content_mask
        && *corner_radii == b.corner_radii
}

/// A path's id, place, colour and vertices; where its builder stood as it
/// added them draws nothing.
fn same_path(a: &Path<ScaledPixels>, b: &Path<ScaledPixels>) -> bool {
    let Path {
        id,
        order,
        bounds,
        content_mask,
        vertices,
        color,
        ..
    } = a;
    *id == b.id
        && *order == b.order
        && *bounds == b.bounds
        && *content_mask == b.content_mask
        && *color == b.color
        && same_all(vertices, &b.vertices, same_vertex)
}

fn same_vertex(a: &PathVertex<ScaledPixels>, b: &PathVertex<ScaledPixels>) -> bool {
    let PathVertex {
        xy_position,
        st_position,
        content_mask,
    } = a;
    *xy_position == b.xy_position
        && *st_position == b.st_position
        && *content_mask == b.content_mask
}

fn same_composition(
    a: &crate::fast::composition::SceneComposition,
    b: &crate::fast::composition::SceneComposition,
) -> bool {
    let crate::fast::composition::SceneComposition {
        placements,
        holes,
        blurs,
        masks,
        // What painting the masks kept as it went.
        open_masks: _,
        mask_ids: _,
    } = a;
    *placements == b.placements
        && same_all(holes, &b.holes, same_quad)
        && *blurs == b.blurs
        && *masks == b.masks
}

/// The same scroll layers, each at the generation of content its tiles were
/// rasterized from, with the same tiles left to rasterize.
fn same_layers(
    a: &crate::fast::layers::scene::SceneLayers,
    b: &crate::fast::layers::scene::SceneLayers,
) -> bool {
    a.frames.len() == b.frames.len()
        && a.frames.iter().zip(&b.frames).all(|(a, b)| {
            let crate::fast::layers::scene::LayerFrame {
                key,
                generation,
                background,
                tile_size,
                // What `generation` stands for.
                content: _,
                dirty_tiles,
            } = a;
            *key == b.key
                && *generation == b.generation
                && *background == b.background
                && *tile_size == b.tile_size
                && *dirty_tiles == b.dirty_tiles
        })
}
