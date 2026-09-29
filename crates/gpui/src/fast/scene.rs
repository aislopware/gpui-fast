//! A scene's paint operations as places in its primitive lists, drawing
//! them again from last frame's lists, putting a finished scene in drawing
//! order without moving its primitives more than once or allocating, and
//! scene helpers for tests: a finished scene described as text, to compare
//! two frames by, and forgetting the orderings the bounds tree replays.
//!
//! Upstream keeps each primitive twice, in its kind's list and in the paint
//! operation that painted it, and drawing a retained subtree again clones
//! every operation's primitive back through [`Scene::insert_primitive`]. Here
//! an operation names the primitive's place in its kind's list in painting
//! order, and drawing it again copies it from there, with the ordering the
//! bounds tree gives the entry it made last frame.

use crate::{
    MonochromeSprite, PaintOperation, PaintSurface, Path, PathId, PolychromeSprite, Primitive,
    PrimitiveKind, Quad, ScaledPixels, Scene, Shadow, SubpixelSprite, Underline,
};
use std::{mem, ops::Range};

/// A painted primitive: its kind, and its place in its kind's list in
/// painting order.
#[derive(Clone, Copy, Debug)]
pub(crate) struct PrimitiveAt {
    kind: PrimitiveKind,
    index: u32,
}

/// No entry in the bounds tree.
const NO_ENTRY: u32 = u32::MAX;

/// What a scene keeps of how it was painted, to be drawn again from.
#[derive(Default)]
pub(crate) struct Painted {
    /// For each paint operation, the entry it made in the bounds tree, or
    /// [`NO_ENTRY`]: a primitive in a paint layer takes the layer's
    /// ordering, and a native's entries are made again with it.
    entries: Vec<u32>,
    /// For each kind, whether finishing the scene gathered its primitives
    /// in drawing order into the kind's list and left them in painting
    /// order in the sort scratch.
    gathered: [bool; KINDS],
}

const KINDS: usize = 8;

fn kind_index(kind: PrimitiveKind) -> usize {
    match kind {
        PrimitiveKind::Shadow => 0,
        PrimitiveKind::Quad => 1,
        PrimitiveKind::Path => 2,
        PrimitiveKind::Underline => 3,
        PrimitiveKind::MonochromeSprite => 4,
        PrimitiveKind::SubpixelSprite => 5,
        PrimitiveKind::PolychromeSprite => 6,
        PrimitiveKind::Surface => 7,
    }
}

/// What [`Scene::clear`] clears of what this module keeps.
pub(crate) fn clear(scene: &mut Scene) {
    scene.fast_painted.entries.clear();
    scene.fast_painted.gathered = [false; KINDS];
}

/// Records `operation`, which made no entry in the bounds tree.
pub(crate) fn push(scene: &mut Scene, operation: PaintOperation) {
    scene.paint_operations.push(operation);
    scene.fast_painted.entries.push(NO_ENTRY);
}

/// Records `operation`, which made the bounds tree's last entry.
pub(crate) fn push_entered(scene: &mut Scene, operation: PaintOperation) {
    scene.paint_operations.push(operation);
    let entry = scene.primitive_bounds.len() as u32 - 1;
    scene.fast_painted.entries.push(entry);
}

/// Records the painting of `primitive`, just pushed onto its kind's list,
/// which made the bounds tree's last entry unless a paint layer is open.
pub(crate) fn push_primitive(scene: &mut Scene, primitive: &Primitive) {
    let (kind, len) = match primitive {
        Primitive::Shadow(_) => (PrimitiveKind::Shadow, scene.shadows.len()),
        Primitive::Quad(_) => (PrimitiveKind::Quad, scene.quads.len()),
        Primitive::Path(_) => (PrimitiveKind::Path, scene.paths.len()),
        Primitive::Underline(_) => (PrimitiveKind::Underline, scene.underlines.len()),
        Primitive::MonochromeSprite(_) => (
            PrimitiveKind::MonochromeSprite,
            scene.monochrome_sprites.len(),
        ),
        Primitive::SubpixelSprite(_) => {
            (PrimitiveKind::SubpixelSprite, scene.subpixel_sprites.len())
        }
        Primitive::PolychromeSprite(_) => (
            PrimitiveKind::PolychromeSprite,
            scene.polychrome_sprites.len(),
        ),
        Primitive::Surface(_) => (PrimitiveKind::Surface, scene.surfaces.len()),
    };
    let at = PaintOperation::Primitive(PrimitiveAt {
        kind,
        index: len as u32 - 1,
    });
    if scene.layer_stack.is_empty() {
        push_entered(scene, at);
    } else {
        push(scene, at);
    }
}

/// Hands `next`, cleared to draw the next frame, the entries the bounds
/// tree of `rendered`, the frame just drawn, made, which the next frame
/// replays; `next` last drew the frame before.
pub(crate) fn take_orderings(next: &mut Scene, rendered: &mut Scene) {
    next.primitive_bounds
        .take_previous(&mut rendered.primitive_bounds);
}

/// What [`Scene::replay`] does: draws the paint operations `range` of
/// `previous`, the scene of the frame before, again.
pub(crate) fn replay(scene: &mut Scene, range: Range<usize>, previous: &Scene) {
    scene.paint_operations.reserve(range.len());
    scene.fast_painted.entries.reserve(range.len());
    for index in range {
        let entry = previous.fast_painted.entries[index];
        match &previous.paint_operations[index] {
            PaintOperation::Primitive(at) => replay_primitive(scene, previous, *at, entry),
            PaintOperation::StartLayer(bounds) => {
                let order = if entry == NO_ENTRY {
                    scene.primitive_bounds.insert(*bounds)
                } else {
                    scene
                        .primitive_bounds
                        .insert_replayed(entry as usize, *bounds)
                };
                scene.layer_stack.push(order);
                push_entered(scene, PaintOperation::StartLayer(*bounds));
            }
            PaintOperation::EndLayer => scene.pop_layer(),
            PaintOperation::Native(placement) => {
                crate::fast::composition::scene::replay(scene, placement)
            }
        }
    }
}

/// Draws again the primitive `at` of `previous`, which made the entry
/// `entry` in its bounds tree.
fn replay_primitive(scene: &mut Scene, previous: &Scene, at: PrimitiveAt, entry: u32) {
    let index = at.index as usize;
    let layer = scene.layer_stack.last().copied();
    macro_rules! replay {
        ($field:ident) => {{
            let kind = kind_index(at.kind);
            let painted = if previous.fast_painted.gathered[kind] {
                &previous.sort_scratch.$field
            } else {
                &previous.$field
            };
            let mut primitive = painted[index].clone();
            primitive.order = match layer {
                Some(order) => order,
                None => {
                    let bounds = primitive.bounds.intersect(&primitive.content_mask.bounds);
                    if entry == NO_ENTRY {
                        scene.primitive_bounds.insert(bounds)
                    } else {
                        scene
                            .primitive_bounds
                            .insert_replayed(entry as usize, bounds)
                    }
                }
            };
            primitive
        }};
    }
    match at.kind {
        PrimitiveKind::Shadow => {
            let shadow = replay!(shadows);
            scene.shadows.push(shadow);
        }
        PrimitiveKind::Quad => {
            let quad = replay!(quads);
            scene.quads.push(quad);
        }
        PrimitiveKind::Path => {
            let mut path = replay!(paths);
            path.id = PathId(scene.paths.len());
            scene.paths.push(path);
        }
        PrimitiveKind::Underline => {
            let underline = replay!(underlines);
            scene.underlines.push(underline);
        }
        PrimitiveKind::MonochromeSprite => {
            let sprite = replay!(monochrome_sprites);
            scene.monochrome_sprites.push(sprite);
        }
        PrimitiveKind::SubpixelSprite => {
            let sprite = replay!(subpixel_sprites);
            scene.subpixel_sprites.push(sprite);
        }
        PrimitiveKind::PolychromeSprite => {
            let sprite = replay!(polychrome_sprites);
            scene.polychrome_sprites.push(sprite);
        }
        PrimitiveKind::Surface => {
            let surface = replay!(surfaces);
            scene.surfaces.push(surface);
        }
    }
    let len = match at.kind {
        PrimitiveKind::Shadow => scene.shadows.len(),
        PrimitiveKind::Quad => scene.quads.len(),
        PrimitiveKind::Path => scene.paths.len(),
        PrimitiveKind::Underline => scene.underlines.len(),
        PrimitiveKind::MonochromeSprite => scene.monochrome_sprites.len(),
        PrimitiveKind::SubpixelSprite => scene.subpixel_sprites.len(),
        PrimitiveKind::PolychromeSprite => scene.polychrome_sprites.len(),
        PrimitiveKind::Surface => scene.surfaces.len(),
    };
    let operation = PaintOperation::Primitive(PrimitiveAt {
        kind: at.kind,
        index: len as u32 - 1,
    });
    if layer.is_some() {
        push(scene, operation);
    } else {
        push_entered(scene, operation);
    }
}

/// Room to sort a scene's primitives in, kept from one frame to the next so
/// that a frame does not allocate megabytes to put what it drew in order.
#[derive(Default)]
pub(crate) struct SortScratch {
    /// Each item's key and index, packed `key << 32 | index`.
    order: Vec<u64>,
    /// The other half of the radix sort's double buffer.
    swap: Vec<u64>,
    shadows: Vec<Shadow>,
    quads: Vec<Quad>,
    paths: Vec<Path<ScaledPixels>>,
    underlines: Vec<Underline>,
    monochrome_sprites: Vec<MonochromeSprite>,
    subpixel_sprites: Vec<SubpixelSprite>,
    polychrome_sprites: Vec<PolychromeSprite>,
    surfaces: Vec<PaintSurface>,
}

/// Puts `items` in the order `key` gives, keeping the order they came in
/// among equal keys, as a stable sort does.
///
/// Sorting indices and then gathering once moves each item a single time,
/// where sorting the items themselves moves them as often as the sort needs
/// to compare them — and these items are 112 to 168 bytes each. Each index
/// is sorted packed with its key, so the sort never reaches into the items,
/// and by radix, in two or three passes over a frame's tens of thousands of
/// glyphs rather than the dozen a comparison sort makes. Items already in
/// order, as most kinds of a frame drawn much like the last one are, are
/// left where they are.
fn sort_by_gathering<T: Clone>(
    items: &mut Vec<T>,
    order: &mut Vec<u64>,
    swap: &mut Vec<u64>,
    gathered: &mut Vec<T>,
    key: impl Fn(&T) -> u64,
) -> bool {
    if items.len() < 2 {
        return false;
    }
    order.clear();
    let mut sorted = true;
    let mut previous = 0;
    let mut max = 0;
    for (index, item) in items.iter().enumerate() {
        let key = key(item);
        sorted &= key >= previous;
        previous = key;
        max = max.max(key);
        order.push(key << 32 | index as u64);
    }
    if sorted {
        return false;
    }
    if max <= u32::MAX as u64 {
        radix_sort_keys(order, swap, u64::BITS - max.leading_zeros());
    } else {
        // Never in practice: a key wider than 32 bits, which the packing
        // cut. Sorted by comparison instead, keys taken afresh.
        order.sort_unstable_by_key(|&packed| {
            let index = packed as u32;
            (key(&items[index as usize]), index)
        });
    }
    gathered.clear();
    gathered.extend(
        order
            .iter()
            .map(|&packed| items[packed as u32 as usize].clone()),
    );
    mem::swap(items, gathered);
    true
}

/// Sorts `packed` by the `bits` of key above each index, stably: a least
/// significant digit radix sort of 11 bits a pass, through `swap`.
fn radix_sort_keys(packed: &mut Vec<u64>, swap: &mut Vec<u64>, bits: u32) {
    const DIGIT: u32 = 11;
    const BUCKETS: usize = 1 << DIGIT;
    let mut counts = [0u32; BUCKETS];
    let mut shift = 32;
    while shift < 32 + bits {
        let digit = |value: u64| (value >> shift) as usize & (BUCKETS - 1);
        counts.fill(0);
        for &value in packed.iter() {
            counts[digit(value)] += 1;
        }
        let mut start = 0;
        for count in &mut counts {
            (*count, start) = (start, start + *count);
        }
        swap.clear();
        swap.resize(packed.len(), 0);
        for &value in packed.iter() {
            let slot = &mut counts[digit(value)];
            swap[*slot as usize] = value;
            *slot += 1;
        }
        mem::swap(packed, swap);
        shift += DIGIT;
    }
}

/// A sprite's place in drawing order: its ordering, then its atlas texture,
/// as one key. Sprites of one ordering never overlap unless a paint layer
/// gave them its ordering, and then they keep the order they were painted
/// in within their texture.
fn sprite_key(order: u32, texture: u32, textures: u32) -> u64 {
    order as u64 * textures as u64 + texture as u64
}

impl Scene {
    /// Forgets the orderings recorded for replaying, so the next frame orders
    /// every primitive from scratch.
    #[cfg(test)]
    pub(crate) fn forget_orderings(&mut self) {
        self.primitive_bounds.forget();
    }

    /// Everything this finished scene draws, in drawing order, as text two
    /// scenes can be compared by: each primitive with its bounds, clip, colours
    /// and ordering, and each layer's bounds. Atlas tiles are left out, since
    /// two windows need not place the same glyph in the same tile.
    #[cfg(test)]
    pub(crate) fn describe(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for operation in &self.paint_operations {
            match operation {
                crate::PaintOperation::StartLayer(bounds) => {
                    lines.push(format!("layer {bounds:?}"))
                }
                crate::PaintOperation::EndLayer => lines.push("end layer".into()),
                crate::PaintOperation::Primitive(..) => {}
                crate::PaintOperation::Native(placement) => lines.push(format!(
                    "native {:?} {} {:?} {:?} {:?} {} hitbox {} focus {}",
                    placement.id,
                    placement.order,
                    placement.bounds,
                    placement.content_mask,
                    placement.corner_radii,
                    placement.opacity,
                    placement.hitbox.is_some(),
                    placement.focus.is_some(),
                )),
            }
        }
        lines.extend(self.shadows.iter().map(|shadow| format!("{shadow:?}")));
        lines.extend(self.quads.iter().map(|quad| format!("{quad:?}")));
        lines.extend(
            self.underlines
                .iter()
                .map(|underline| format!("{underline:?}")),
        );
        lines.extend(self.monochrome_sprites.iter().map(|sprite| {
            format!(
                "monochrome sprite {} {:?} {:?} {:?}",
                sprite.order, sprite.bounds, sprite.content_mask, sprite.color
            )
        }));
        lines.extend(self.subpixel_sprites.iter().map(|sprite| {
            format!(
                "subpixel sprite {} {:?} {:?} {:?}",
                sprite.order, sprite.bounds, sprite.content_mask, sprite.color
            )
        }));
        lines.extend(self.polychrome_sprites.iter().map(|sprite| {
            format!(
                "polychrome sprite {} {:?} {:?}",
                sprite.order, sprite.bounds, sprite.content_mask
            )
        }));
        lines.extend(
            self.paths
                .iter()
                .map(|path| format!("path {} {:?}", path.order, path.bounds)),
        );
        lines
    }
}

/// What [`Scene::finish`] does: puts every primitive in drawing order.
/// Sprites of one order are grouped by the atlas texture they come from:
/// a batch draws from one texture, and tile ids, which each texture
/// numbers from zero, would interleave them.
#[inline]
pub(crate) fn sort_in_drawing_order(scene: &mut Scene) {
    let scratch = &mut scene.sort_scratch;
    let gathered = &mut scene.fast_painted.gathered;
    macro_rules! sort {
        ($field:ident, $kind:expr, $key:expr) => {{
            gathered[kind_index($kind)] = sort_by_gathering(
                &mut scene.$field,
                &mut scratch.order,
                &mut scratch.swap,
                &mut scratch.$field,
                $key,
            );
        }};
    }
    macro_rules! sort_sprites {
        ($field:ident, $kind:expr) => {{
            let textures = scene
                .$field
                .iter()
                .map(|sprite| sprite.tile.texture_id.index)
                .max()
                .map_or(1, |max| max + 1);
            sort!($field, $kind, |sprite| sprite_key(
                sprite.order,
                sprite.tile.texture_id.index,
                textures
            ));
        }};
    }
    sort!(
        shadows,
        PrimitiveKind::Shadow,
        |shadow: &Shadow| shadow.order as u64
    );
    sort!(quads, PrimitiveKind::Quad, |quad: &Quad| quad.order as u64);
    sort!(paths, PrimitiveKind::Path, |path: &Path<ScaledPixels>| {
        path.order as u64
    });
    sort!(
        underlines,
        PrimitiveKind::Underline,
        |underline: &Underline| { underline.order as u64 }
    );
    sort_sprites!(monochrome_sprites, PrimitiveKind::MonochromeSprite);
    sort_sprites!(subpixel_sprites, PrimitiveKind::SubpixelSprite);
    sort_sprites!(polychrome_sprites, PrimitiveKind::PolychromeSprite);
    sort!(
        surfaces,
        PrimitiveKind::Surface,
        |surface: &PaintSurface| { surface.order as u64 }
    );
    scene.composition.sort();
}

#[cfg(test)]
mod tests {
    use super::sort_by_gathering;
    use rand::{Rng as _, SeedableRng as _, rngs::StdRng};

    /// Sorts as a stable sort by key does, whether the keys are narrow, wide
    /// enough to take several radix passes, too wide to pack, or in order.
    #[test]
    fn sorting_by_gathering_is_a_stable_sort_by_key() {
        let mut rng = StdRng::seed_from_u64(7);
        let (mut order, mut swap, mut gathered) = (Vec::new(), Vec::new(), Vec::new());
        for (len, max_key) in [
            (0, 1),
            (1, 1),
            (50, 3),
            (5_000, 40),
            (5_000, 1 << 20),
            (5_000, u32::MAX as u64),
            (300, u64::MAX >> 1),
        ] {
            for presorted in [false, true] {
                let mut items: Vec<(u64, usize)> = (0..len)
                    .map(|index| (rng.random_range(0..=max_key), index))
                    .collect();
                if presorted {
                    items.sort_by_key(|item| item.0);
                }
                let mut expected = items.clone();
                expected.sort_by_key(|item| item.0);
                sort_by_gathering(&mut items, &mut order, &mut swap, &mut gathered, |item| {
                    item.0
                });
                assert_eq!(items, expected, "{len} items, keys up to {max_key}");
            }
        }
    }
}
