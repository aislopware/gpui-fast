//! A coarse picture of a window each frame, for the region rules: the
//! frame's scene rasterized on the CPU into cells a few points across, or a
//! rendered image averaged into the same cells. Each cell holds its mean
//! colour in OKLab, over black.
//!
//! The raster is approximate where precision costs and nothing a person sees
//! change is lost: corners are square, a glyph covers a share of its quad, a
//! shadow is half its colour over its bounds, a border is its colour over the
//! share of the quad it covers, a gradient is the mean of its stops and an
//! image a grey that names its atlas tile, so a swapped image changes it.
//! Scroll layers' tiles, backdrop blurs and masks draw nothing of their own.
//! What it keeps is a surface's colour where it sits, at its opacity.
//!
//! Rune's raster of its painter stream is the model (anomalyco/rune,
//! `crates/rune/src/motion_gate/check/raster.rs`; MIT).

use image::RgbaImage;

use crate::{Bounds, Hsla, Rgba, ScaledPixels, Scene};

/// Points per cell.
pub const CELL: f32 = 12.;

/// One frame's cells, row-major, in OKLab.
#[derive(Clone, Debug, PartialEq)]
pub struct Grid {
    /// Cells across.
    pub width: usize,
    /// Cells down.
    pub height: usize,
    lab: Vec<[f32; 3]>,
}

/// One shape to cover cells with: `rect` in device pixels, straight linear
/// `color` with its alpha, covering `density` of the rect.
struct Draw {
    rect: Bounds<ScaledPixels>,
    color: [f32; 4],
    density: f32,
}

impl Grid {
    /// The cells of a window `width` × `height` points across.
    fn cells(width: f32, height: f32) -> (usize, usize) {
        (
            (width / CELL).ceil().max(1.) as usize,
            (height / CELL).ceil().max(1.) as usize,
        )
    }

    /// Cell `index`'s colour in OKLab.
    pub fn lab(&self, index: usize) -> [f32; 3] {
        self.lab[index]
    }

    /// Cell `index`'s colour as `#rrggbb`, for findings.
    pub fn hex(&self, index: usize) -> String {
        let [r, g, b] =
            oklab_to_linear(self.lab[index]).map(|c| (linear_to_srgb(c) * 255.).round() as u8);
        format!("#{r:02x}{g:02x}{b:02x}")
    }

    /// `scene`, drawn into a window `width` × `height` points across at
    /// `scale` device pixels a point.
    pub fn of_scene(scene: &Scene, width: f32, height: f32, scale: f32) -> Grid {
        let (w, h) = Self::cells(width, height);
        let side = CELL * scale;
        let mut cells = vec![[0f32; 3]; w * h];
        for draw in draws(scene) {
            let rect = draw.rect;
            if rect.size.width.0 <= 0. || rect.size.height.0 <= 0. || draw.color[3] <= 0. {
                continue;
            }
            let (x0, y0) = (rect.origin.x.0, rect.origin.y.0);
            let (x1, y1) = (x0 + rect.size.width.0, y0 + rect.size.height.0);
            let cx0 = (x0 / side).floor().max(0.) as usize;
            let cy0 = (y0 / side).floor().max(0.) as usize;
            let cx1 = ((x1 / side).ceil().max(0.) as usize).min(w);
            let cy1 = ((y1 / side).ceil().max(0.) as usize).min(h);
            for cy in cy0..cy1 {
                for cx in cx0..cx1 {
                    let (left, top) = (cx as f32 * side, cy as f32 * side);
                    let covered = (x1.min(left + side) - x0.max(left)).max(0.)
                        * (y1.min(top + side) - y0.max(top)).max(0.)
                        / (side * side);
                    let alpha = (draw.color[3] * draw.density * covered).clamp(0., 1.);
                    for (cell, color) in cells[cy * w + cx].iter_mut().zip(draw.color) {
                        *cell = *cell * (1. - alpha) + color * alpha;
                    }
                }
            }
        }
        Grid {
            width: w,
            height: h,
            lab: cells.into_iter().map(linear_to_oklab).collect(),
        }
    }

    /// A rendered frame, `image` in sRGB with straight alpha over black, for
    /// a window `width` × `height` points across.
    pub fn of_image(image: &RgbaImage, width: f32, height: f32) -> Grid {
        let (w, h) = Self::cells(width, height);
        let (px_w, px_h) = (image.width() as f32 / width, image.height() as f32 / height);
        let mut sums = vec![[0f32; 4]; w * h];
        for (x, y, pixel) in image.enumerate_pixels() {
            let cx = ((x as f32 / px_w / CELL) as usize).min(w - 1);
            let cy = ((y as f32 / px_h / CELL) as usize).min(h - 1);
            let alpha = pixel[3] as f32 / 255.;
            let sum = &mut sums[cy * w + cx];
            for (total, channel) in sum.iter_mut().zip(&pixel.0[..3]) {
                *total += srgb_to_linear(f32::from(*channel) / 255.) * alpha;
            }
            sum[3] += 1.;
        }
        Grid {
            width: w,
            height: h,
            lab: sums
                .into_iter()
                .map(|sum| {
                    let n = sum[3].max(1.);
                    linear_to_oklab([sum[0] / n, sum[1] / n, sum[2] / n])
                })
                .collect(),
        }
    }
}

/// What `scene` draws, in drawing order.
fn draws(scene: &Scene) -> Vec<Draw> {
    // (order, kind, draw): kinds in a batch's order within one draw order.
    let mut out: Vec<(u32, u8, Draw)> = Vec::new();
    let clip = |bounds: &Bounds<ScaledPixels>, mask: &Bounds<ScaledPixels>| bounds.intersect(mask);
    for shadow in &scene.shadows {
        if shadow.inset == 0 {
            out.push((
                shadow.order,
                0,
                Draw {
                    rect: clip(&shadow.bounds, &shadow.content_mask.bounds),
                    color: linear(shadow.color),
                    density: 0.5,
                },
            ));
        }
    }
    for quad in &scene.quads {
        let rect = clip(&quad.bounds, &quad.content_mask.bounds);
        out.push((
            quad.order,
            1,
            Draw {
                rect,
                color: background(&quad.background),
                density: 1.,
            },
        ));
        let widths = &quad.border_widths;
        let (w, h) = (quad.bounds.size.width.0, quad.bounds.size.height.0);
        let border = (widths.top.0 + widths.bottom.0) * w + (widths.left.0 + widths.right.0) * h;
        if border > 0. && w * h > 0. {
            out.push((
                quad.order,
                1,
                Draw {
                    rect,
                    color: linear(quad.border_color),
                    density: (border / (w * h)).min(1.),
                },
            ));
        }
    }
    for path in &scene.paths {
        out.push((
            path.order,
            2,
            Draw {
                rect: clip(&path.bounds, &path.content_mask.bounds),
                color: background(&path.color),
                density: 0.5,
            },
        ));
    }
    for underline in &scene.underlines {
        out.push((
            underline.order,
            3,
            Draw {
                rect: clip(&underline.bounds, &underline.content_mask.bounds),
                color: linear(underline.color),
                density: 1.,
            },
        ));
    }
    for sprite in &scene.monochrome_sprites {
        out.push((
            sprite.order,
            4,
            Draw {
                rect: clip(&sprite.bounds, &sprite.content_mask.bounds),
                color: linear(sprite.color),
                density: 0.35,
            },
        ));
    }
    for sprite in &scene.subpixel_sprites {
        out.push((
            sprite.order,
            5,
            Draw {
                rect: clip(&sprite.bounds, &sprite.content_mask.bounds),
                color: linear(sprite.color),
                density: 0.35,
            },
        ));
    }
    for sprite in &scene.polychrome_sprites {
        // A grey that names the tile, so another image changes the cells.
        let tile = sprite.tile.tile_id.0.wrapping_mul(2654435761) ^ sprite.tile.texture_id.index;
        let grey = 0.2 + (tile % 61) as f32 / 100.;
        out.push((
            sprite.order,
            6,
            Draw {
                rect: clip(&sprite.bounds, &sprite.content_mask.bounds),
                color: [grey, grey, grey, sprite.opacity],
                density: 1.,
            },
        ));
    }
    for surface in &scene.surfaces {
        out.push((
            surface.order,
            7,
            Draw {
                rect: clip(&surface.bounds, &surface.content_mask.bounds),
                color: [0.5, 0.5, 0.5, surface.opacity],
                density: 1.,
            },
        ));
    }
    out.sort_by_key(|(order, kind, _)| (*order, *kind));
    out.into_iter().map(|(_, _, draw)| draw).collect()
}

/// A background's colour: a gradient's or a pattern's the mean of its stops.
fn background(background: &crate::Background) -> [f32; 4] {
    match background.tag {
        crate::color::BackgroundTag::Solid => linear(background.solid),
        _ => {
            let [a, b] = background.colors.map(|stop| linear(stop.color));
            std::array::from_fn(|i| (a[i] + b[i]) / 2.)
        }
    }
}

/// `color` in linear sRGB, with its alpha.
fn linear(color: Hsla) -> [f32; 4] {
    let rgba = Rgba::from(color);
    [
        srgb_to_linear(rgba.r),
        srgb_to_linear(rgba.g),
        srgb_to_linear(rgba.b),
        rgba.a,
    ]
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f32) -> f32 {
    let c = c.clamp(0., 1.);
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1. / 2.4) - 0.055
    }
}

/// Björn Ottosson's OKLab, from linear sRGB.
fn linear_to_oklab([r, g, b]: [f32; 3]) -> [f32; 3] {
    let l = (0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
    [
        0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
        1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s,
        0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s,
    ]
}

fn oklab_to_linear([l, a, b]: [f32; 3]) -> [f32; 3] {
    let l_ = (l + 0.396_337_78 * a + 0.215_803_76 * b).powi(3);
    let m_ = (l - 0.105_561_346 * a - 0.063_854_17 * b).powi(3);
    let s_ = (l - 0.089_484_18 * a - 1.291_485_5 * b).powi(3);
    [
        4.076_741_7 * l_ - 3.307_711_6 * m_ + 0.230_969_94 * s_,
        -1.268_438 * l_ + 2.609_757_4 * m_ - 0.341_319_38 * s_,
        -0.004_196_086_3 * l_ - 0.703_418_6 * m_ + 1.707_614_7 * s_,
    ]
}
