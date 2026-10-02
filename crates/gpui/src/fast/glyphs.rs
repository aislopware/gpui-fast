//! Glyph painting that works out a run's rendering once, not once a glyph.

use crate::{
    App, AtlasTile, Bounds, ContentMask, DecorationRun, DevicePixels, FontId, GlyphId, Hsla,
    IsZero, LineLayout, MonochromeSprite, Pixels, Point, RenderGlyphParams, SUBPIXEL_VARIANTS_X,
    SUBPIXEL_VARIANTS_Y, ScaledPixels, ShapedRun, Size, SubpixelSprite, TransformationMatrix,
    Window, util::round_half_toward_zero,
};
use anyhow::Result;
use std::borrow::Cow;

/// A glyph's whole-pixel origin and subpixel variant: upstream's rounding
/// for non-negative coordinates, extended so that a whole-pixel shift moves
/// the result by exactly that shift everywhere, negative coordinates included.
///
/// Upstream rounds `origin * variants` half toward zero, then takes `trunc`
/// and `fract`, which mirror around zero; a glyph painted above or left of
/// the window (a scroll layer's overscan) would then land on a different
/// pixel or variant than the same glyph painted once scrolled into view.
pub(crate) fn quantize_origin(origin: Point<ScaledPixels>) -> (Point<ScaledPixels>, Point<u8>) {
    let (x, variant_x) = quantize_axis(origin.x.0, SUBPIXEL_VARIANTS_X);
    let (y, variant_y) = quantize_axis(origin.y.0, SUBPIXEL_VARIANTS_Y);
    (
        Point::new(ScaledPixels(x), ScaledPixels(y)),
        Point::new(variant_x, variant_y),
    )
}

/// The whole pixel at or below `value` plus the nearest of `variants` steps
/// within it, ties toward the pixel; the last step carries into the next pixel.
/// On non-negative values `floor` is `trunc` and the steps round like
/// upstream's `round_half_toward_zero(value * variants)`.
fn quantize_axis(value: f32, variants: u8) -> (f32, u8) {
    let whole = value.floor();
    let steps = round_half_toward_zero((value - whole) * variants as f32) as i32;
    let carry = steps / variants as i32;
    (whole + carry as f32, (steps % variants as i32) as u8)
}

/// An emoji's whole-pixel origin: the nearest whole pixel, ties toward the
/// pixel below, which is upstream's `round_half_toward_zero` on non-negative
/// values and shifts with whole-pixel moves on negative ones.
pub(crate) fn quantize_emoji_origin(origin: Point<ScaledPixels>) -> Point<ScaledPixels> {
    origin.map(|c| {
        let whole = c.0.floor();
        ScaledPixels(whole + round_half_toward_zero(c.0 - whole))
    })
}

/// [`LineGlyphPainter::meets_mask`] for a glyph on its own, in a font whose
/// bounding box is `font_box` and whose ascent and descent are given.
#[cfg(test)]
pub(crate) fn may_reach(
    font: (Bounds<Pixels>, Pixels, Pixels),
    origin: Point<Pixels>,
    baseline: Pixels,
    mask: &Bounds<Pixels>,
) -> bool {
    let (font_box, ascent, descent) = font;
    GlyphReach::new(&GlyphExtent::new(font_box, ascent, descent), mask)
        .contains(origin.x, origin.y + baseline)
}

/// How far a font's glyphs may draw from where they are placed, at one size:
/// left of and right of the glyph's origin, above and below its baseline,
/// each plus a margin for glyph dilation and subpixel positioning.
///
/// It is the font's bounding box, which encloses every glyph of the font,
/// placed on the baseline as the font's y-up coordinates put it, widened to
/// the font's ascent and descent: not every text system's bounding box is the
/// font's (cosmic-text's starts on the baseline and is ascent plus descent
/// tall, which leaves out descenders).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct GlyphExtent {
    before: Pixels,
    after: Pixels,
    above: Pixels,
    below: Pixels,
}

impl GlyphExtent {
    const MARGIN: Pixels = Pixels(2.);

    fn new(font_box: Bounds<Pixels>, ascent: Pixels, descent: Pixels) -> Self {
        let zero = Pixels::ZERO;
        Self {
            before: (-font_box.origin.x).max(zero) + Self::MARGIN,
            after: (font_box.origin.x + font_box.size.width).max(zero) + Self::MARGIN,
            above: (font_box.origin.y + font_box.size.height).max(ascent) + Self::MARGIN,
            below: (-font_box.origin.y).max(-descent).max(zero) + Self::MARGIN,
        }
    }
}

/// [`LineGlyphPainter::meets_mask`] for the glyphs of a run: the mask's
/// edges pushed out by the glyphs' [`GlyphExtent`], which a line works out
/// once a run, so that each glyph only compares its origin and baseline with
/// them.
#[derive(Clone, Copy)]
struct GlyphReach {
    left: Pixels,
    top: Pixels,
    right: Pixels,
    bottom: Pixels,
}

impl GlyphReach {
    /// A reach nothing is inside, for a line whose first run has not begun.
    const NOWHERE: Self = Self {
        left: Pixels(f32::INFINITY),
        top: Pixels(f32::INFINITY),
        right: Pixels(f32::NEG_INFINITY),
        bottom: Pixels(f32::NEG_INFINITY),
    };

    fn new(extent: &GlyphExtent, mask: &Bounds<Pixels>) -> Self {
        // An empty mask's edges would let in the glyphs straddling it.
        if mask.is_empty() {
            return Self::NOWHERE;
        }
        let end = mask.bottom_right();
        Self {
            left: mask.origin.x - extent.after,
            top: mask.origin.y - extent.below,
            right: end.x + extent.before,
            bottom: end.y + extent.above,
        }
    }

    /// Whether a glyph whose origin is at `x` and whose baseline is at `y`
    /// may draw inside the mask.
    #[inline]
    fn contains(&self, x: Pixels, y: Pixels) -> bool {
        y > self.top && y < self.bottom && x > self.left && x < self.right
    }
}

/// How the glyphs of a run are rendered: what painting a glyph needs that
/// depends on its run, not on the glyph. See [`Window::glyph_run_rendering`].
#[derive(Clone, Copy, PartialEq)]
pub(crate) struct GlyphRunRendering {
    subpixel_rendering: bool,
    dilation: u8,
}

impl Window {
    /// How the glyphs of a run in `font_id` at `font_size` and in `color` are
    /// rendered, which [`Window::paint_glyph_in_run`] takes so that painting a
    /// line works it out once a run rather than once a glyph: it asks the
    /// window how it is drawn and converts the colour to find its dilation.
    pub(crate) fn glyph_run_rendering(
        &self,
        font_id: FontId,
        font_size: Pixels,
        color: Hsla,
    ) -> GlyphRunRendering {
        GlyphRunRendering {
            subpixel_rendering: self.should_use_subpixel_rendering(font_id, font_size),
            dilation: crate::fast::text_smoothing::dilation(self, color),
        }
    }

    /// [`Window::paint_glyph`], for a glyph in a run whose rendering and
    /// snapped content mask the caller has already worked out.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn paint_glyph_in_run(
        &mut self,
        origin: Point<Pixels>,
        font_id: FontId,
        glyph_id: GlyphId,
        font_size: Pixels,
        color: Hsla,
        rendering: GlyphRunRendering,
        content_mask: ContentMask<ScaledPixels>,
    ) -> Result<()> {
        self.invalidator.debug_assert_paint();

        let element_opacity = self.element_opacity();
        let scale_factor = self.scale_factor();
        let glyph_origin = origin.scale(scale_factor);
        crate::fast::scene::glyph_at(&mut self.next_frame.scene, glyph_origin);

        let (integer_origin, subpixel_variant) = quantize_origin(glyph_origin);
        let GlyphRunRendering {
            subpixel_rendering,
            dilation,
        } = rendering;
        let params = RenderGlyphParams {
            font_id,
            glyph_id,
            font_size,
            subpixel_variant,
            scale_factor,
            is_emoji: false,
            subpixel_rendering,
            dilation,
        };

        let (raster_bounds, tile, sprite_size) = match self.fast_glyph_bounds.lookup(&params) {
            Some(kept) => kept,
            None => {
                let raster_bounds = self.text_system().raster_bounds(&params)?;
                self.fast_glyph_bounds.insert(&params, raster_bounds);
                (raster_bounds, None, None)
            }
        };
        let sprite_origin = integer_origin + raster_bounds.origin.map(Into::into);
        if !raster_bounds.is_zero() {
            let tile = match tile {
                Some(tile) => tile,
                // The scene drops a sprite that misses its content mask;
                // where the sprite's size is known, drop it before looking up
                // its tile.
                None if sprite_size.is_some_and(|size| {
                    Bounds::new(sprite_origin, size.map(Into::into))
                        .intersect(&content_mask.bounds)
                        .is_empty()
                }) =>
                {
                    return Ok(());
                }
                None => {
                    let tile = self
                        .sprite_atlas
                        .get_or_insert_with(params.clone().into(), &mut || {
                            let (size, bytes) = self.text_system().rasterize_glyph(&params)?;
                            Ok(Some((size, Cow::Owned(bytes))))
                        })?
                        .expect("Callback above only errors or returns Some");
                    self.fast_glyph_bounds.insert_tile(&params, tile);
                    tile
                }
            };
            let bounds = Bounds {
                origin: sprite_origin,
                size: tile.bounds.size.map(Into::into),
            };

            if subpixel_rendering {
                self.next_frame.scene.insert_primitive(SubpixelSprite {
                    order: 0,
                    pad: 0,
                    bounds,
                    content_mask,
                    color: color.opacity(element_opacity),
                    tile,
                    transformation: TransformationMatrix::unit(),
                });
            } else {
                self.next_frame.scene.insert_primitive(MonochromeSprite {
                    order: 0,
                    pad: 0,
                    bounds,
                    content_mask,
                    color: color.opacity(element_opacity),
                    tile,
                    transformation: TransformationMatrix::unit(),
                });
            }
        }
        Ok(())
    }
}

/// Paints the glyphs of a line, working out what they share once rather than
/// once a glyph: the content mask, which nothing painted along a line
/// changes, snapped for its sprites and pushed out by each run's glyph
/// extent for [`LineGlyphPainter::meets_mask`], and the rendering of each
/// run, which only changes with the run's font or color.
pub(crate) struct LineGlyphPainter {
    snapped_content_mask: ContentMask<ScaledPixels>,
    content_mask: Bounds<Pixels>,
    reach: GlyphReach,
    run_rendering: Option<(FontId, Hsla, GlyphRunRendering)>,
}

impl LineGlyphPainter {
    pub(crate) fn new(window: &Window) -> Self {
        Self {
            snapped_content_mask: window.snapped_content_mask(),
            content_mask: window.content_mask().bounds,
            reach: GlyphReach::NOWHERE,
            run_rendering: None,
        }
    }

    /// Whether the run's next glyph may draw inside the line's content
    /// mask, for the line to skip the glyphs it need not paint. `line_glyph`
    /// is the line's glyph box as upstream builds it, whose origin is the
    /// glyph's origin on the top of its line; the glyph itself is drawn
    /// `baseline` further down, on the line's baseline, where a tall line
    /// puts it well below the top. Upstream's `line_glyph.intersects(mask)`
    /// takes the box at the top of the line, and skips glyphs low on a tall
    /// line that reach into the mask.
    ///
    /// This test is only conservative, leaving the exact test to the scene,
    /// which drops a sprite outside its content mask: it takes the run's
    /// [`GlyphExtent`] around the glyph's origin on the baseline. Nothing
    /// reaches into an empty mask. Where a glyph is drawn therefore depends
    /// on its sprite alone, so a line painted in a scroll layer's overscan
    /// and scrolled into view shows the same glyphs as the line painted in
    /// place.
    #[inline]
    pub(crate) fn meets_mask(&self, line_glyph: &Bounds<Pixels>, baseline: Pixels) -> bool {
        self.reach
            .contains(line_glyph.origin.x, line_glyph.origin.y + baseline)
    }

    /// [`Window::paint_glyph`], for the next glyph of the line.
    pub(crate) fn paint_glyph(
        &mut self,
        window: &mut Window,
        origin: Point<Pixels>,
        font_id: FontId,
        glyph_id: GlyphId,
        font_size: Pixels,
        color: Hsla,
    ) -> Result<()> {
        let rendering = match self.run_rendering {
            Some((run_font_id, run_color, rendering))
                if run_font_id == font_id && run_color == color =>
            {
                rendering
            }
            _ => {
                let rendering = window.glyph_run_rendering(font_id, font_size, color);
                self.run_rendering = Some((font_id, color, rendering));
                rendering
            }
        };
        window.paint_glyph_in_run(
            origin,
            font_id,
            glyph_id,
            font_size,
            color,
            rendering,
            self.snapped_content_mask,
        )
    }
}

/// How many glyphs' raster bounds a window keeps at hand, as a power of two.
/// See [`GlyphBoundsCache`].
const GLYPH_BOUNDS_SLOT_BITS: u32 = 12;

/// The raster bounds of the glyphs a window painted lately, so painting a
/// glyph needn't ask the text system, which locks its map of every glyph's
/// raster bounds and hashes the glyph's whole description to look it up. A
/// glyph's raster bounds only depend on that description, so the answer is
/// the text system's own. Each glyph has one slot it can be kept in; a glyph
/// that needs a slot another holds takes it over.
///
/// A glyph's slot also keeps where the glyph is in the window's sprite atlas,
/// for the rest of the frame it was looked up in: the same digits are painted
/// in hundreds of places a frame, and each lookup locks the atlas and hashes
/// the glyph's description again. A tile is only kept for the frame, since
/// the atlas may be cleared when the frame is presented (after a run of GPU
/// errors, or when the device is lost), but glyphs are never removed from it
/// while a frame is painted.
///
/// The bounding boxes of the fonts lines were painted in lately, at the sizes
/// they were painted at, are kept here too. See [`bounding_box`].
pub(crate) struct GlyphBoundsCache {
    slots: Box<[Option<GlyphSlot>]>,
    /// Counts the frames the window finished painting, telling a tile looked
    /// up in this one from one looked up before.
    frame: u64,
    font_extents: Vec<(FontId, Pixels, Bounds<Pixels>, GlyphExtent)>,
}

/// What [`GlyphBoundsCache`] keeps of a glyph.
#[derive(Clone)]
struct GlyphSlot {
    params: RenderGlyphParams,
    raster_bounds: Bounds<DevicePixels>,
    /// The glyph's tile, and the frame it was looked up in; kept past that
    /// frame for its size.
    tile: Option<(u64, AtlasTile)>,
}

impl Default for GlyphBoundsCache {
    fn default() -> Self {
        Self {
            slots: vec![None; 1 << GLYPH_BOUNDS_SLOT_BITS].into_boxed_slice(),
            frame: 0,
            font_extents: Vec::new(),
        }
    }
}

impl GlyphBoundsCache {
    /// The slot a glyph is kept in, from everything that tells apart the
    /// glyphs a window paints: the same digit in two sizes, or in two colors
    /// dilated differently, or at another subpixel offset, would otherwise
    /// keep taking each other's slot.
    fn slot(params: &RenderGlyphParams) -> usize {
        const K: u64 = 0x9e37_79b9_7f4a_7c15;
        let words = [
            params.glyph_id.0 as u64 | (params.font_id.0 as u64) << 32,
            params.font_size.0.to_bits() as u64
                | (params.subpixel_variant.x as u64) << 32
                | (params.subpixel_variant.y as u64) << 40
                | (params.dilation as u64) << 48
                | (params.subpixel_rendering as u64) << 56
                | (params.is_emoji as u64) << 57,
            params.scale_factor.to_bits() as u64,
        ];
        let hash = words.iter().fold(0u64, |hash, word| {
            (hash.rotate_left(26) ^ word).wrapping_mul(K)
        });
        (hash >> (64 - GLYPH_BOUNDS_SLOT_BITS)) as usize
    }

    /// The glyph's raster bounds, if kept.
    #[cfg(test)]
    pub(crate) fn get(&self, params: &RenderGlyphParams) -> Option<Bounds<DevicePixels>> {
        self.lookup(params).map(|(bounds, _, _)| bounds)
    }

    /// The glyph's raster bounds, if kept, its tile, if it was looked up this
    /// frame, and the size of its sprite, if its tile was ever looked up: a
    /// glyph's rasterization, and so its tile's size, only depends on its
    /// description, whether or not the atlas still holds it.
    pub(crate) fn lookup(
        &self,
        params: &RenderGlyphParams,
    ) -> Option<(
        Bounds<DevicePixels>,
        Option<AtlasTile>,
        Option<Size<DevicePixels>>,
    )> {
        match &self.slots[Self::slot(params)] {
            Some(slot) if slot.params == *params => Some((
                slot.raster_bounds,
                slot.tile
                    .and_then(|(frame, tile)| (frame == self.frame).then_some(tile)),
                slot.tile.map(|(_, tile)| tile.bounds.size),
            )),
            _ => None,
        }
    }

    pub(crate) fn insert(&mut self, params: &RenderGlyphParams, bounds: Bounds<DevicePixels>) {
        self.slots[Self::slot(params)] = Some(GlyphSlot {
            params: params.clone(),
            raster_bounds: bounds,
            tile: None,
        });
    }

    /// Keeps the tile the glyph was found at in the sprite atlas this frame,
    /// if the glyph is kept.
    pub(crate) fn insert_tile(&mut self, params: &RenderGlyphParams, tile: AtlasTile) {
        if let Some(slot) = &mut self.slots[Self::slot(params)]
            && slot.params == *params
        {
            slot.tile = Some((self.frame, tile));
        }
    }

    /// Ends the frame the tiles kept were looked up in.
    pub(crate) fn finish_frame(&mut self) {
        self.frame += 1;
    }
}

/// Begins painting `run` of `layout` with `painter`, and returns the size of
/// the run's font's bounding box, which a line takes for the width of its
/// glyphs where it has no glyph to measure.
pub(crate) fn begin_run(
    painter: &mut LineGlyphPainter,
    window: &mut Window,
    cx: &App,
    run: &ShapedRun,
    layout: &LineLayout,
) -> Size<Pixels> {
    let (font_box, extent) = font_extent(window, cx, run.font_id, layout.font_size);
    painter.reach = GlyphReach::new(&extent, &painter.content_mask);
    font_box.size
}

/// How many fonts' extents [`font_extent`] keeps, most recent last.
const FONT_EXTENTS: usize = 16;

/// [`TextSystem::bounding_box`](crate::TextSystem::bounding_box) and the
/// [`GlyphExtent`] worked out from it, which painting a line asks for once a
/// run: the text system takes a lock and hashes the font to find its
/// metrics, where a window paints its text in a handful of fonts and sizes,
/// whose extents it keeps.
#[inline]
fn font_extent(
    window: &mut Window,
    cx: &App,
    font_id: FontId,
    font_size: Pixels,
) -> (Bounds<Pixels>, GlyphExtent) {
    let extents = &mut window.fast_glyph_bounds.font_extents;
    if let Some((_, _, font_box, extent)) = extents
        .iter()
        .rev()
        .find(|(id, size, _, _)| *id == font_id && *size == font_size)
    {
        return (*font_box, *extent);
    }
    let text_system = cx.text_system();
    let font_box = text_system.bounding_box(font_id, font_size);
    let extent = GlyphExtent::new(
        font_box,
        text_system.ascent(font_id, font_size),
        text_system.descent(font_id, font_size),
    );
    if extents.len() == FONT_EXTENTS {
        extents.remove(0);
    }
    extents.push((font_id, font_size, font_box, extent));
    (font_box, extent)
}

/// Whether none of a line's decoration runs has a background, so painting its
/// background paints nothing: it needn't walk its glyphs, nor push a layer to
/// paint nothing in, which costs the scene a bounds tree insertion a line.
#[inline]
pub(crate) fn has_no_background(decoration_runs: &[DecorationRun]) -> bool {
    decoration_runs
        .iter()
        .all(|run| run.background_color.is_none())
}
