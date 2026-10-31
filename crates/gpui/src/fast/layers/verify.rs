//! Checking scroll layers against drawing from scratch: a scene's composited
//! tiles expanded into the content they were rasterized from, and what the
//! scene draws as text two scenes drawing the same pixels share. Used by the
//! layer oracle (`fast/tests/layers_oracle.rs`) and by `gpui_perf --verify`
//! through [`Window::painted_primitives`].

use std::collections::HashMap;

use crate::{
    AtlasTextureId, AtlasTile, BackdropBlur, Bounds, LayerFrame, LayerKey, Point, PolychromeSprite,
    Quad, ScaledPixels, Scene, TileCoord, TileId, Window, decode_layer_tile,
    fast::scene::Operation, point, scene::Primitive,
};

/// Every primitive `scene` draws, in the order the renderer draws them:
/// by draw order, then by kind in the order batches of one draw order are
/// drawn, then in the order they were inserted.
fn drawing_sequence(scene: &Scene) -> Vec<Primitive> {
    ordered_sequence(scene)
        .into_iter()
        .map(|(_, primitive)| primitive)
        .collect()
}

/// [`drawing_sequence`], each primitive with its draw order.
fn ordered_sequence(scene: &Scene) -> Vec<(u32, Primitive)> {
    let mut sequence: Vec<((u32, u8, usize), Primitive)> = Vec::new();
    macro_rules! gather {
        ($field:ident, $rank:expr, $variant:ident) => {
            sequence.extend(
                scene
                    .$field
                    .iter()
                    .enumerate()
                    .map(|(ix, p)| ((p.order, $rank, ix), Primitive::$variant(p.clone()))),
            )
        };
    }
    gather!(shadows, 0, Shadow);
    gather!(quads, 1, Quad);
    gather!(paths, 2, Path);
    gather!(underlines, 3, Underline);
    gather!(monochrome_sprites, 4, MonochromeSprite);
    gather!(subpixel_sprites, 5, SubpixelSprite);
    gather!(polychrome_sprites, 6, PolychromeSprite);
    gather!(surfaces, 7, Surface);
    sequence.sort_by_key(|(key, _)| *key);
    sequence
        .into_iter()
        .map(|((order, ..), primitive)| (order, primitive))
        .collect()
}

/// What a scene draws, in drawing order: a primitive, or a backdrop blur
/// between the primitives drawn under it and those drawn over it.
enum Drawn {
    /// A primitive, and for one of a layer's content with a fade of its
    /// own the fade it is drawn in, moved and composed (see
    /// [`crate::fast::edge_fade::moved_fade`]).
    Primitive(Primitive, Option<crate::EdgeFadeRamps>),
    Blur(BackdropBlur),
}

impl Drawn {
    /// Where it draws, or for a blur where it reads from too.
    fn reach(&self) -> Bounds<ScaledPixels> {
        match self {
            Drawn::Primitive(primitive, _) => clipped(primitive),
            Drawn::Blur(blur) => blur.reach(),
        }
    }

    fn describe(&self, scene: &Scene) -> String {
        match self {
            Drawn::Primitive(primitive, None) => describe(primitive, scene),
            Drawn::Primitive(primitive, Some(fade)) => {
                let mut primitive = primitive.clone();
                crate::fast::edge_fade::unfade(&mut primitive);
                format!(
                    "{}{}",
                    describe_unfaded(&primitive),
                    crate::fast::edge_fade::describe_ramps(fade)
                )
            }
            Drawn::Blur(blur) => format!(
                "backdrop blur {:?} {:?} {:?} {:?} {}{}",
                blur.bounds,
                blur.content_mask,
                blur.corner_radii,
                blur.blur_radius,
                blur.opacity,
                crate::fast::edge_fade::describe(scene, blur.fade),
            ),
        }
    }
}

/// Whether `primitive` leaves every pixel under it as it was: its colour, and its border's
/// where it has one, wholly transparent.
fn draws_nothing(primitive: &Primitive) -> bool {
    match primitive {
        Primitive::Shadow(p) => p.color.is_transparent(),
        Primitive::Quad(p) => {
            p.background.is_transparent()
                && (p.border_color.is_transparent() || p.border_widths == Default::default())
        }
        Primitive::Path(p) => p.color.is_transparent(),
        Primitive::Underline(p) => p.color.is_transparent(),
        Primitive::MonochromeSprite(p) => p.color.is_transparent(),
        Primitive::SubpixelSprite(p) => p.color.is_transparent(),
        Primitive::PolychromeSprite(p) => p.opacity <= 0.,
        Primitive::Surface(p) => p.opacity <= 0.,
    }
}

fn clipped(primitive: &Primitive) -> Bounds<ScaledPixels> {
    primitive
        .bounds()
        .intersect(&primitive.content_mask().bounds)
}

/// `primitive` clipped to `mask` as well as to its own mask.
fn clip_to(primitive: &mut Primitive, mask: &Bounds<ScaledPixels>) {
    let content_mask = match primitive {
        Primitive::Shadow(p) => &mut p.content_mask,
        Primitive::Quad(p) => &mut p.content_mask,
        Primitive::Path(p) => &mut p.content_mask,
        Primitive::Underline(p) => &mut p.content_mask,
        Primitive::MonochromeSprite(p) => &mut p.content_mask,
        Primitive::SubpixelSprite(p) => &mut p.content_mask,
        Primitive::PolychromeSprite(p) => &mut p.content_mask,
        Primitive::Surface(p) => &mut p.content_mask,
    };
    content_mask.bounds = content_mask.bounds.intersect(mask);
}

/// The drawing sequence of `layer`'s content moved by `translation` and
/// clipped to `viewport`, as the renderer draws it when it composites the
/// layer's tiles. Checks that every piece of it lies under one of the tiles
/// in `composited`.
fn expand_layer(
    layer: &LayerFrame,
    translation: Point<ScaledPixels>,
    viewport: &Bounds<ScaledPixels>,
    composited: &[TileCoord],
) -> Vec<Primitive> {
    let mut scratch = Scene::default();
    for operation in layer.content.operations() {
        match operation {
            Operation::Primitive(primitive) => {
                let mut moved =
                    crate::fast::layers::scene::translate_primitive(&primitive, translation);
                clip_to(&mut moved, viewport);
                scratch.insert_primitive(moved);
            }
            Operation::StartLayer(bounds) => scratch.push_layer(Bounds {
                origin: bounds.origin + translation,
                size: bounds.size,
            }),
            Operation::EndLayer => scratch.pop_layer(),
            // Content placing a native or a blur is never composited from
            // tiles.
            Operation::Native(_) | Operation::Backdrop(_) => {}
        }
    }
    scratch.finish();
    let sequence = drawing_sequence(&scratch);
    let side = layer.tile_size as f32;
    let first = |at: ScaledPixels| (at.0 / side).floor() as i32;
    let last = |start: ScaledPixels, end: ScaledPixels| {
        ((end.0 / side).ceil() as i32 - 1).max(first(start))
    };
    for primitive in &sequence {
        let window = clipped(primitive);
        let start = point(
            window.origin.x - translation.x,
            window.origin.y - translation.y,
        );
        let end = point(start.x + window.size.width, start.y + window.size.height);
        for y in first(start.y)..=last(start.y, end.y) {
            for x in first(start.x)..=last(start.x, end.x) {
                assert!(
                    composited.contains(&TileCoord { x, y }),
                    "no tile composited at ({x}, {y}) of layer {:?}, under {}",
                    layer.key,
                    describe_unfaded(primitive)
                );
            }
        }
    }
    sequence
}

/// What `scene` draws, with every composited scroll layer's tiles replaced
/// by the content they were rasterized from, in drawing order.
fn expanded_sequence(scene: &Scene) -> Vec<Drawn> {
    let mut composited: HashMap<
        LayerKey,
        (
            Point<ScaledPixels>,
            Bounds<ScaledPixels>,
            Vec<TileCoord>,
            u32,
        ),
    > = HashMap::new();
    let mut first_tiles: Vec<(usize, LayerKey)> = Vec::new();
    let mut sequence = Vec::new();
    let mut blurs = scene.backdrop_blurs().iter().peekable();
    for (order, primitive) in ordered_sequence(scene) {
        while let Some(blur) = blurs.next_if(|blur| blur.order <= order) {
            sequence.push(Drawn::Blur(*blur));
        }
        if let Primitive::PolychromeSprite(sprite) = &primitive
            && let Some((key, tile)) =
                decode_layer_tile(sprite.tile.texture_id, sprite.tile.tile_id)
        {
            let layer = layer_frame(scene, key);
            let origin = layer.tile_bounds(tile).origin;
            let translation = point(
                sprite.bounds.origin.x - origin.x,
                sprite.bounds.origin.y - origin.y,
            );
            let viewport = sprite.content_mask.bounds;
            let entry = composited.entry(key).or_insert_with(|| {
                first_tiles.push((sequence.len(), key));
                (translation, viewport, Vec::new(), sprite.pad)
            });
            assert_eq!(
                (entry.0, entry.1, entry.3),
                (translation, viewport, sprite.pad),
                "tiles of layer {key:?} composited at different offsets, clips or fades"
            );
            entry.2.push(tile);
            continue;
        }
        sequence.push(Drawn::Primitive(primitive, None));
    }
    sequence.extend(blurs.map(|blur| Drawn::Blur(*blur)));
    for (at, key) in first_tiles.into_iter().rev() {
        let (translation, viewport, tiles, fade) = &composited[&key];
        let content = expand_layer(layer_frame(scene, key), *translation, viewport, tiles);
        // The edge fade the tiles were composited in fades their content,
        // around any fade it set of its own, which moves with it.
        let content = content.into_iter().map(|mut primitive| {
            let own = crate::fast::edge_fade::fade_of(&primitive);
            let moved = crate::fast::edge_fade::moved_fade(scene, *fade, own, *translation);
            crate::fast::edge_fade::stamp_with(*fade, &mut primitive);
            Drawn::Primitive(primitive, moved)
        });
        sequence.splice(at..at, content);
    }
    sequence
}

fn layer_frame(scene: &Scene, key: LayerKey) -> &LayerFrame {
    scene
        .layers
        .frames
        .iter()
        .find(|layer| layer.key == key)
        .unwrap_or_else(|| panic!("a tile of layer {key:?}, which the scene has no frame of"))
}

/// A primitive of `scene` as text, without its draw order, where its atlas
/// put its raster or the number its edge fade has in the window's table,
/// which two windows drawing the same thing may number differently: its
/// fade is told by its ramps.
fn describe(primitive: &Primitive, scene: &Scene) -> String {
    let fade = crate::fast::edge_fade::describe(scene, crate::fast::edge_fade::fade_of(primitive));
    let mut primitive = primitive.clone();
    crate::fast::edge_fade::unfade(&mut primitive);
    format!("{}{fade}", describe_unfaded(&primitive))
}

fn describe_unfaded(primitive: &Primitive) -> String {
    fn neutral(tile: AtlasTile) -> AtlasTile {
        AtlasTile {
            texture_id: AtlasTextureId {
                index: 0,
                kind: tile.texture_id.kind,
            },
            tile_id: TileId(0),
            padding: tile.padding,
            bounds: Bounds::default(),
        }
    }
    match primitive {
        Primitive::Shadow(p) => format!("{:?}", crate::Shadow { order: 0, ..*p }),
        Primitive::Quad(p) => format!("{:?}", Quad { order: 0, ..*p }),
        // Only what a renderer reads of a vertex: its position and its curve coordinates.
        Primitive::Path(p) => format!(
            "path {:?} {:?} {:?} {:?}",
            p.bounds,
            p.content_mask,
            p.color,
            p.vertices
                .iter()
                .map(|v| (v.xy_position, v.st_position))
                .collect::<Vec<_>>()
        ),
        Primitive::Underline(p) => format!("{:?}", crate::Underline { order: 0, ..*p }),
        Primitive::MonochromeSprite(p) => format!(
            "{:?}",
            crate::MonochromeSprite {
                order: 0,
                tile: neutral(p.tile),
                transformation: settled(p.transformation),
                ..*p
            }
        ),
        Primitive::SubpixelSprite(p) => format!(
            "{:?}",
            crate::SubpixelSprite {
                order: 0,
                tile: neutral(p.tile),
                transformation: settled(p.transformation),
                ..*p
            }
        ),
        Primitive::PolychromeSprite(p) => format!(
            "{:?}",
            PolychromeSprite {
                order: 0,
                tile: neutral(p.tile),
                ..*p
            }
        ),
        Primitive::Surface(p) => format!(
            "surface {:?} {:?} opacity {}",
            p.bounds, p.content_mask, p.opacity
        ),
    }
}

/// A sprite's transform as two frames placing it alike both give it: its
/// translation on the 1/64 pixel grid and its rotation and scale to 1/4096,
/// without a negative zero. A rotated sprite's translation is worked out from
/// where its element landed, and a frame that drew the element again and one
/// that kept it reach the same place through different sums, a few ulps apart
/// (a spinner at a quarter turn: -591.0 and -590.99994).
fn settled(transform: crate::TransformationMatrix) -> crate::TransformationMatrix {
    fn on(grid: f32) -> impl Fn(f32) -> f32 {
        move |value| (value * grid).round() / grid + 0.0
    }
    crate::TransformationMatrix {
        rotation_scale: transform.rotation_scale.map(|row| row.map(on(4096.0))),
        translation: transform.translation.map(on(64.0)),
    }
}

/// What `scene` draws, as text that two scenes drawing the same pixels share.
///
/// The drawing sequence, layers expanded, is put in a canonical order: each
/// primitive's depth is one more than the deepest primitive drawn before it
/// that it overlaps, and primitives are sorted by depth, then by text. Two
/// sequences that draw every overlapping pair in the same order get the same
/// depths, however they number their draw orders and whatever order they
/// draw primitives that do not overlap in.
pub(crate) fn drawn(scene: &Scene) -> Vec<String> {
    let mut sequence = expanded_sequence(scene);
    // What draws no pixel is no part of what a scene shows: text or an icon faded to nothing,
    // painted by one frame and skipped by another, draws the same pixels.
    sequence.retain(
        |drawn| !matches!(drawn, Drawn::Primitive(primitive, _) if draws_nothing(primitive)),
    );
    let clips: Vec<Bounds<ScaledPixels>> = sequence.iter().map(Drawn::reach).collect();
    let mut depths: Vec<u32> = Vec::with_capacity(sequence.len());
    for (ix, clip) in clips.iter().enumerate() {
        let mut depth = 0;
        for (earlier, earlier_clip) in clips[..ix].iter().enumerate() {
            if depths[earlier] + 1 > depth && clip.intersects(earlier_clip) {
                depth = depths[earlier] + 1;
            }
        }
        depths.push(depth);
    }
    let mut lines: Vec<(u32, String)> = depths
        .into_iter()
        .zip(sequence.iter().map(|drawn| drawn.describe(scene)))
        .collect();
    lines.sort();
    lines
        .into_iter()
        .map(|(depth, line)| format!("{depth} {line}"))
        .collect()
}

impl Window {
    /// What the last frame drew, one line per primitive and backdrop blur,
    /// each composited scroll layer's tiles replaced by the content they were
    /// rasterized from, in a canonical drawing order that does not depend on draw order
    /// numbers or atlas placement. Two windows drawing the same pixels
    /// return the same lines, with or without scroll layers.
    ///
    /// Panics if the scene composites a layer's content without the tile it
    /// lies in.
    pub fn painted_primitives(&self) -> Vec<String> {
        drawn(&self.rendered_frame.scene)
    }

    /// The surfaces the last frame drew, in the order the scene holds them,
    /// with the buffer each samples: for a test asking which picture reached
    /// the frame.
    pub fn painted_surfaces(&self) -> Vec<crate::PaintSurface> {
        self.rendered_frame.scene.surfaces.clone()
    }
}

/// Where a primitive a scene draws comes from: a paint operation of the
/// scene's own, by its index in painting order, or a part of a composited
/// layer's content, by the scene of that part (see
/// [`crate::fast::layers::scene::LayerContent`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    Operation(usize),
    Layer(LayerKey, *const Scene),
}

/// A primitive or backdrop blur a scene draws, as [`drawn`] describes it
/// but without its depth, where it comes from and where it draws.
pub(crate) struct Sourced {
    pub(crate) text: String,
    pub(crate) source: Source,
    pub(crate) reach: Bounds<ScaledPixels>,
}

/// What `scene` draws, as [`drawn`] gives it, each primitive without its
/// depth but with where it comes from, in no particular order: every paint
/// operation of the scene's own, and for each composited layer the content
/// its tiles were rasterized from, part by part.
pub(crate) fn sourced(scene: &Scene) -> Vec<Sourced> {
    let mut composited: Vec<(LayerKey, Point<ScaledPixels>, Bounds<ScaledPixels>, u32)> =
        Vec::new();
    let mut out = Vec::new();
    for (index, operation) in crate::fast::scene::operations(scene).enumerate() {
        let drawn = match operation {
            Operation::Primitive(Primitive::PolychromeSprite(sprite))
                if decode_layer_tile(sprite.tile.texture_id, sprite.tile.tile_id).is_some() =>
            {
                let (key, tile) = decode_layer_tile(sprite.tile.texture_id, sprite.tile.tile_id)
                    .expect("checked above");
                if !composited.iter().any(|(k, ..)| *k == key) {
                    let origin = layer_frame(scene, key).tile_bounds(tile).origin;
                    let translation = point(
                        sprite.bounds.origin.x - origin.x,
                        sprite.bounds.origin.y - origin.y,
                    );
                    composited.push((key, translation, sprite.content_mask.bounds, sprite.pad));
                }
                continue;
            }
            Operation::Primitive(primitive) => Drawn::Primitive(primitive, None),
            Operation::Backdrop(blur) => Drawn::Blur(blur),
            Operation::StartLayer(_) | Operation::EndLayer | Operation::Native(_) => continue,
        };
        push_sourced(&mut out, scene, drawn, Source::Operation(index));
    }
    for (key, translation, viewport, fade) in composited {
        let layer = layer_frame(scene, key);
        for part in layer.content.scenes() {
            for operation in crate::fast::scene::operations(part) {
                let Operation::Primitive(primitive) = operation else {
                    continue;
                };
                let mut primitive =
                    crate::fast::layers::scene::translate_primitive(&primitive, translation);
                clip_to(&mut primitive, &viewport);
                let own = crate::fast::edge_fade::fade_of(&primitive);
                let moved = crate::fast::edge_fade::moved_fade(scene, fade, own, translation);
                crate::fast::edge_fade::stamp_with(fade, &mut primitive);
                push_sourced(
                    &mut out,
                    scene,
                    Drawn::Primitive(primitive, moved),
                    Source::Layer(key, part as *const Scene),
                );
            }
        }
    }
    out
}

fn push_sourced(out: &mut Vec<Sourced>, scene: &Scene, drawn: Drawn, source: Source) {
    if matches!(&drawn, Drawn::Primitive(primitive, _) if draws_nothing(primitive)) {
        return;
    }
    let reach = drawn.reach();
    if reach.size.width.0 <= 0. || reach.size.height.0 <= 0. {
        return;
    }
    out.push(Sourced {
        text: drawn.describe(scene),
        source,
        reach,
    });
}

#[cfg(test)]
mod tests {
    use super::describe_unfaded;
    use crate::{
        AtlasTextureId, AtlasTextureKind, AtlasTile, Bounds, ContentMask, MonochromeSprite, TileId,
        TransformationMatrix, hsla, point, px, scene::Primitive, size,
    };

    fn spinner(translation: [f32; 2], turn: f32) -> Primitive {
        let bounds = Bounds::new(point(px(160.), px(751.)), size(px(42.), px(42.))).scale(1.0);
        Primitive::MonochromeSprite(MonochromeSprite {
            order: 0,
            pad: 0,
            bounds,
            content_mask: ContentMask { bounds },
            color: hsla(0.66, 0.04, 0.4, 1.),
            tile: AtlasTile {
                texture_id: AtlasTextureId {
                    index: 3,
                    kind: AtlasTextureKind::Monochrome,
                },
                tile_id: TileId(7),
                padding: 0,
                bounds: Bounds::default(),
            },
            transformation: TransformationMatrix {
                rotation_scale: [[turn, 1.0], [-1.0, turn]],
                translation,
            },
        })
    }

    /// A sprite one frame placed a few ulps from where another did reads the
    /// same; one a pixel away does not.
    #[test]
    fn a_sprite_placed_a_few_ulps_apart_reads_the_same() {
        let kept = describe_unfaded(&spinner([-591.0, 953.0], 0.0));
        let again = describe_unfaded(&spinner([-590.99994, 952.99994], -1.2e-8));
        assert_eq!(kept, again);
        let moved = describe_unfaded(&spinner([-590.0, 953.0], 0.0));
        assert_ne!(kept, moved);
    }
}
