//! Keyed paint: a stretch of an element's paint that the caller names by a
//! key, drawn again from last frame, in place or moved, while the key is the
//! same.
//!
//! Element retention ([`crate::fast::element`]) draws `div`s and text again
//! by comparing what they were built with. An element that paints itself —
//! a terminal's grid, a chart, a code view — builds nothing that could be
//! compared, and paints every frame: a terminal of 200 × 60 cells paints
//! 12 000 glyphs again for a blinking cursor. [`Window::paint_keyed`] lets
//! such an element name a stretch of what it paints, a row say, by a key
//! that stands for everything the stretch paints relative to an origin. A
//! stretch whose key was painted last frame is copied from last frame's
//! scene rather than painted: as it was when the origin, the content mask,
//! the opacity and whether it lies in a paint layer are as they were, and
//! moved (see [`crate::fast::shift`]) when only the origin moved, by whole
//! device pixels, as a row does when the output scrolls.
//!
//! The key is the caller's promise: two stretches under one key paint the
//! same primitives relative to their origins. Keys are one space per window,
//! so a stretch painted by one element may be drawn again for another; that
//! is sound by the promise, and keys that must not meet fold in what tells
//! them apart. A stretch registers nothing but primitives: one that
//! registered a mouse listener, an input handler, a cursor style, a tab stop
//! or a window control, read element state, or laid out text is painted
//! afresh every frame, since drawing it again would leave those out.
//!
//! Drawing a stretch again copies its paint operations from last frame's
//! scene ([`crate::fast::scene::replay`]), so its primitives keep their
//! atlas tiles: a key covers any texture its stretch paints that can be
//! removed between frames, as an image can.

use crate::fast::scene::{begin_noting, end_noting};
use crate::fast::shift::{
    Masks, Noted, Shift, ShiftedOperation, known_inside, replay_shifted, shift_operations,
    still_beyond,
};
use crate::{Bounds, Pixels, Point, ScaledPixels, Window, px};
use collections::FxHashMap;
use std::ops::Range;

/// The stretches a scene holds painted under keys, for the next frame to
/// draw again. A stretch drawn again with what holds it, a view retained
/// whole, is carried into the next scene with it ([`KeyedPaints::carry`]),
/// so an element painted every few frames finds its stretches. Carrying is
/// a copy, as retained views are drawn again every frame; the stretches are
/// found by key only once an element paints under one.
#[derive(Default)]
pub(crate) struct KeyedPaints {
    /// The stretches, by where they begin in the scene's operations.
    stretches: Vec<Stretch>,
    /// Each key's first stretch in `stretches`, made when a stretch is first
    /// looked up; `None` until then.
    index: Option<FxHashMap<u64, u32>>,
    /// Room for the operations of a stretch drawn again moved, kept from
    /// frame to frame.
    moved: Vec<ShiftedOperation>,
}

impl KeyedPaints {
    pub(crate) fn clear(&mut self) {
        self.stretches.clear();
        if let Some(index) = &mut self.index {
            index.clear();
        }
        self.moved.clear();
    }

    /// Keeps `stretch`, by where it begins.
    fn insert(&mut self, stretch: Stretch) {
        // A stretch ends after those nested in it, and is kept before them.
        let at = if self
            .stretches
            .last()
            .is_none_or(|last| last.start <= stretch.start)
        {
            self.stretches.len()
        } else {
            self.stretches
                .partition_point(|kept| kept.start <= stretch.start)
        };
        self.stretches.insert(at, stretch);
    }

    /// The first stretch kept under `key`.
    fn find(&mut self, key: u64) -> Option<Stretch> {
        if self.stretches.is_empty() {
            return None;
        }
        let stretches = &self.stretches;
        let index = self.index.get_or_insert_with(FxHashMap::default);
        if index.is_empty() {
            index.reserve(stretches.len());
            for (at, stretch) in stretches.iter().enumerate() {
                index.entry(stretch.key).or_insert(at as u32);
            }
        }
        index.get(&key).map(|&at| stretches[at as usize])
    }

    /// Takes into `next` the stretches of `previous` lying inside `range`
    /// of its operations, which `next` has drawn again from `to` on.
    pub(crate) fn carry(next: &mut Self, previous: &Self, range: Range<usize>, to: usize) {
        if previous.stretches.is_empty() {
            return;
        }
        let (from, to) = (range.start as u32, to as u32);
        let end = range.end as u32;
        let first = previous.stretches.partition_point(|kept| kept.start < from);
        for stretch in &previous.stretches[first..] {
            if stretch.start >= end {
                break;
            }
            // A stretch that painted nothing, all of it culled, lies between
            // two operations; at an end of `range` it may be of what was
            // painted beside it rather than inside, and is left to be
            // painted again, or two ranges drawn again would each carry it.
            let inside = if stretch.start == stretch.end {
                from < stretch.start
            } else {
                stretch.end <= end
            };
            if inside {
                next.insert(Stretch {
                    start: stretch.start - from + to,
                    end: stretch.end - from + to,
                    ..*stretch
                });
            }
        }
    }
}

/// What one stretch painted, and where.
#[derive(Clone, Copy)]
struct Stretch {
    key: u64,
    /// Its paint operations in the frame's scene, `start..end`.
    start: u32,
    end: u32,
    context: Context,
    /// What painting it noted, for it to be moved.
    noted: Noted,
    /// Whether it pushed a content mask of its own.
    clips: bool,
}

/// Where a stretch is painted.
#[derive(Clone, Copy, PartialEq)]
struct Context {
    origin: Point<Pixels>,
    mask: Bounds<Pixels>,
    opacity: f32,
    layered: bool,
    scale_factor: f32,
}

impl Window {
    /// Paints what `paint` paints, or draws it again from last frame.
    ///
    /// `key` stands for everything `paint` paints relative to `origin`:
    /// while a stretch painted under `key` last frame is found, `paint` is
    /// not called, and what it painted is drawn again, where it was if
    /// `origin` is where it was, and moved with `origin` when that moved by
    /// whole device pixels and the move is exact. Otherwise `paint` is
    /// called and what it paints is kept under `key` for the next frame.
    ///
    /// `paint` should paint primitives only (quads, glyphs, sprites,
    /// underlines, shadows, paint layers and content masks): a stretch that
    /// registers a listener, an input handler, a cursor style or a tab stop,
    /// reads element state or lays out text is painted every frame. A key
    /// must change with anything its stretch paints, including a texture
    /// that can be removed from the atlas between frames.
    ///
    /// This method should only be called as part of the paint phase of
    /// element drawing.
    pub fn paint_keyed(&mut self, key: u64, origin: Point<Pixels>, paint: impl FnOnce(&mut Window)) {
        self.invalidator.debug_assert_paint();
        let context = Context {
            origin,
            mask: self.content_mask().bounds,
            opacity: self.element_opacity(),
            layered: !self.next_frame.scene.layer_stack.is_empty(),
            scale_factor: self.scale_factor(),
        };
        if self.retained_state.view_retention
            && !self.refreshing
            && let Some(previous) = self.rendered_frame.scene.fast_painted.keyed.find(key)
        {
            if self.draw_keyed_again(&previous, context) {
                return;
            }
        }

        let start = self.paint_index();
        let pushes = self.retained_state.content_mask_pushes;
        let around = begin_noting(&mut self.next_frame.scene, true);
        paint(self);
        let noted = end_noting(&mut self.next_frame.scene, around);
        let end = self.paint_index();
        let stats = &mut self.layout_engine.as_mut().unwrap().retention.stats;
        stats.paints_keyed += 1;
        let registered = start.fast_window_control_hitboxes_index
            != end.fast_window_control_hitboxes_index
            || start.mouse_listeners_index != end.mouse_listeners_index
            || start.input_handlers_index != end.input_handlers_index
            || start.cursor_styles_index != end.cursor_styles_index
            || start.tab_handle_index != end.tab_handle_index
            || start.accessed_element_states_index != end.accessed_element_states_index
            || start.line_layout_index != end.line_layout_index;
        if !registered {
            let clips = self.retained_state.content_mask_pushes != pushes;
            self.next_frame.scene.fast_painted.keyed.insert(Stretch {
                key,
                start: start.scene_index as u32,
                end: end.scene_index as u32,
                context,
                noted,
                clips,
            });
        }
    }

    /// Draws `previous`, last frame's stretch under `key`, again in
    /// `context`, if it can be, and keeps it under `key` for the next frame.
    fn draw_keyed_again(&mut self, previous: &Stretch, context: Context) -> bool {
        let old = previous.context;
        if old.opacity != context.opacity
            || old.layered != context.layered
            || old.scale_factor != context.scale_factor
        {
            return false;
        }
        let start = self.next_frame.scene.paint_operations.len();
        let operations = previous.start as usize..previous.end as usize;
        let noted = if old.origin == context.origin && old.mask == context.mask {
            // Drawing it again carries it, and what is nested in it, along,
            // unless it painted nothing.
            if operations.is_empty() {
                self.next_frame.scene.fast_painted.keyed.insert(Stretch {
                    start: start as u32,
                    end: start as u32,
                    ..*previous
                });
            } else {
                crate::fast::scene::replay(
                    &mut self.next_frame.scene,
                    operations,
                    &self.rendered_frame.scene,
                );
            }
            crate::fast::scene::add_noted(&mut self.next_frame.scene, previous.noted);
            self.layout_engine
                .as_mut()
                .unwrap()
                .retention
                .stats
                .paints_replayed += 1;
            return true;
        } else {
            let Some(shift) = self.keyed_shift(previous, &context) else {
                return false;
            };
            let moved = &mut self.next_frame.scene.fast_painted.keyed.moved;
            moved.clear();
            if !shift_operations(&self.rendered_frame.scene, operations, &shift, moved) {
                return false;
            }
            let moved = std::mem::take(&mut self.next_frame.scene.fast_painted.keyed.moved);
            replay_shifted(&mut self.next_frame.scene, &moved);
            self.next_frame.scene.fast_painted.keyed.moved = moved;
            self.layout_engine
                .as_mut()
                .unwrap()
                .retention
                .stats
                .paints_moved += 1;
            previous.noted.moved(&shift)
        };
        crate::fast::scene::add_noted(&mut self.next_frame.scene, noted);
        let end = self.next_frame.scene.paint_operations.len();
        self.layout_engine
            .as_mut()
            .unwrap()
            .retention
            .stats
            .paints_replayed += 1;
        self.next_frame.scene.fast_painted.keyed.insert(Stretch {
            start: start as u32,
            end: end as u32,
            context,
            noted,
            ..*previous
        });
        true
    }

    /// How `previous` moves to be painted in `context`, if moving it paints
    /// what painting it afresh there would: by whole device pixels, its
    /// glyphs clear of the window's edges, and clipped as it would be.
    fn keyed_shift(&self, previous: &Stretch, context: &Context) -> Option<Shift> {
        let scale_factor = context.scale_factor;
        let by = context.origin - previous.context.origin;
        let offset = by.scale(scale_factor);
        if offset.x.0.fract() != 0. || offset.y.0.fract() != 0. {
            return None;
        }
        let noted = &previous.noted;
        if !noted.glyphs_clear_of_zero(offset) {
            return None;
        }
        let old = previous.context.mask;
        let new = context.mask;
        let mask_moved = Bounds {
            origin: old.origin + by,
            ..old
        } == new;
        let masks = if mask_moved {
            Masks::Moved
        } else {
            // A mask the stretch pushed may be the one around it, which
            // stood still where the stretch moved.
            if previous.clips {
                return None;
            }
            let old_cover = self.cover_bounds(old);
            let new_cover = self.cover_bounds(new);
            let (sides, gap) = noted.culled_sides();
            // What the mask around left out, it may no longer.
            let known = noted
                .culled_in()
                .is_none_or(|culled| known_inside(&culled, offset, &old_cover, &new_cover))
                || still_beyond(sides, px(gap / scale_factor), by, &old, &new)
                    && still_beyond(sides, ScaledPixels(gap), offset, &old_cover, &new_cover);
            if !known {
                return None;
            }
            Masks::Replaced {
                old: old_cover,
                new: new_cover,
            }
        };
        Some(Shift { offset, by, masks })
    }
}

/// Counts a content mask pushed, for a keyed stretch to tell whether it
/// pushed one of its own.
#[inline(always)]
pub(crate) fn content_mask_pushed(window: &mut Window) {
    window.retained_state.content_mask_pushes =
        window.retained_state.content_mask_pushes.wrapping_add(1);
}
