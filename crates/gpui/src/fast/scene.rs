//! Putting a finished scene in drawing order without moving its primitives
//! more than once or allocating, and scene helpers for tests: a finished scene
//! described as text, to compare two frames by, and forgetting the orderings
//! the bounds tree replays.

use crate::{
    MonochromeSprite, PaintSurface, Path, PolychromeSprite, Quad, ScaledPixels, Scene, Shadow,
    SubpixelSprite, Underline,
};
use std::mem;

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
) {
    if items.len() < 2 {
        return;
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
        return;
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
    macro_rules! sort {
        ($field:ident, $key:expr) => {
            sort_by_gathering(
                &mut scene.$field,
                &mut scratch.order,
                &mut scratch.swap,
                &mut scratch.$field,
                $key,
            )
        };
    }
    macro_rules! sort_sprites {
        ($field:ident) => {{
            let textures = scene
                .$field
                .iter()
                .map(|sprite| sprite.tile.texture_id.index)
                .max()
                .map_or(1, |max| max + 1);
            sort!($field, |sprite| sprite_key(
                sprite.order,
                sprite.tile.texture_id.index,
                textures
            ));
        }};
    }
    sort!(shadows, |shadow: &Shadow| shadow.order as u64);
    sort!(quads, |quad: &Quad| quad.order as u64);
    sort!(paths, |path: &Path<ScaledPixels>| path.order as u64);
    sort!(underlines, |underline: &Underline| underline.order as u64);
    sort_sprites!(monochrome_sprites);
    sort_sprites!(subpixel_sprites);
    sort_sprites!(polychrome_sprites);
    sort!(surfaces, |surface: &PaintSurface| surface.order as u64);
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
