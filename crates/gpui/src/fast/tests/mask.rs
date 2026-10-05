//! A mask its caller rasterises ([`Window::paint_mask`]) paints at exact
//! device pixels, its origin rounded to one, and is rasterised once for its
//! key and size however often it is painted.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::{
    Context, DevicePixels, IntoElement, Pixels, Point, Render, ScaledPixels, TestAppContext,
    TransformationMatrix, Window, WindowHandle, canvas, hsla, point, prelude::*, px, size,
};

/// A 13 pt symbol at 2x: 26 by 24 device pixels.
const SIDE: (i32, i32) = (26, 24);

struct Symbol {
    origin: Point<Pixels>,
    /// How the mask is turned: as a chevron turning while a row opens.
    turn: TransformationMatrix,
    /// How many times the mask was rasterised.
    rasters: Rc<Cell<u32>>,
    /// What painting a mask of the wrong length said.
    wrong: Rc<RefCell<Option<String>>>,
}

impl Render for Symbol {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let (origin, rasters, wrong) = (self.origin, self.rasters.clone(), self.wrong.clone());
        let turn = self.turn;
        canvas(
            |_, _, _| {},
            move |_, (), window, _| {
                let side = size(DevicePixels(SIDE.0), DevicePixels(SIDE.1));
                let ink = hsla(0., 0., 0., 1.);
                window
                    .paint_mask(
                        origin,
                        side,
                        "chevron.right 13 regular @2".into(),
                        turn,
                        ink,
                        || {
                            rasters.set(rasters.get() + 1);
                            Ok(Some(vec![255; (SIDE.0 * SIDE.1) as usize]))
                        },
                    )
                    .unwrap();
                let short = window.paint_mask(
                    origin,
                    side,
                    "short".into(),
                    TransformationMatrix::unit(),
                    ink,
                    || Ok(Some(vec![255; 3])),
                );
                *wrong.borrow_mut() = short.err().map(|e| e.to_string());
                let none = size(DevicePixels(0), DevicePixels(SIDE.1));
                window
                    .paint_mask(
                        origin,
                        none,
                        "empty".into(),
                        TransformationMatrix::unit(),
                        ink,
                        || unreachable!("an empty mask is never rasterised"),
                    )
                    .unwrap();
            },
        )
        .size_full()
    }
}

#[crate::test]
fn a_mask_paints_at_its_device_size_and_is_rasterised_once(cx: &mut TestAppContext) {
    let rasters = Rc::new(Cell::new(0));
    let wrong = Rc::new(RefCell::new(None));
    let window: WindowHandle<Symbol> = cx.add_window({
        let (rasters, wrong) = (rasters.clone(), wrong.clone());
        move |_, _| Symbol {
            origin: point(px(10.3), px(5.2)),
            turn: TransformationMatrix::unit(),
            rasters,
            wrong,
        }
    });
    let sprite = |cx: &mut TestAppContext| {
        cx.update_window(window.into(), |_, window, cx| {
            window.draw(cx).clear(cx);
            assert_eq!(window.scale_factor(), 2.0, "the test window's Retina");
            let sprites = &window.rendered_frame.scene.monochrome_sprites;
            assert_eq!(
                sprites.len(),
                1,
                "the one mask; the short and empty ones paint nothing"
            );
            (
                sprites[0].bounds,
                sprites[0].tile.bounds.size,
                sprites[0].transformation,
            )
        })
        .unwrap()
    };
    let (bounds, tile, at_rest) = sprite(cx);
    assert_eq!(at_rest, TransformationMatrix::unit(), "upright at rest");
    // 10.3 and 5.2 points are 20.6 and 10.4 device pixels: rounded to one.
    assert_eq!(bounds.origin, point(ScaledPixels(21.), ScaledPixels(10.)));
    assert_eq!(
        bounds.size,
        size(ScaledPixels(26.), ScaledPixels(24.)),
        "not scaled"
    );
    assert_eq!(
        tile,
        size(DevicePixels(26), DevicePixels(24)),
        "no supersample"
    );
    let said = wrong.borrow().clone().expect("a short mask is an error");
    assert!(said.contains("624 bytes, not 3"), "{said}");

    for x in [11.0, 12.0, 13.0] {
        window
            .update(cx, |symbol, _, cx| {
                symbol.origin = point(px(x), px(5.));
                cx.notify();
            })
            .unwrap();
        let (bounds, _, _) = sprite(cx);
        assert_eq!(
            bounds.origin.x,
            ScaledPixels(x * 2.),
            "painted where it moved"
        );
    }
    assert_eq!(rasters.get(), 1, "rasterised once for its key and size");

    let quarter = TransformationMatrix::unit().rotate(crate::radians(std::f32::consts::FRAC_PI_2));
    window
        .update(cx, |symbol, _, cx| {
            symbol.turn = quarter;
            cx.notify();
        })
        .unwrap();
    let (_, _, turned) = sprite(cx);
    assert_eq!(turned, quarter, "a turning mask carries its turn");
    assert_eq!(rasters.get(), 1, "turning draws the same pixels");
}
