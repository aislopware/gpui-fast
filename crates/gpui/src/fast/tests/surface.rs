//! Video surfaces in views drawn from the last frame. A surface's picture is a
//! `CVPixelBuffer` a decoder hands the view showing it, from outside GPUI; the
//! frame has to show the buffer the view holds now, and a view around it being
//! drawn again must not bring back an older one.

#![cfg(target_os = "macos")]

use crate::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    TestAppContext, Window, WindowHandle, div, px, surface,
};
use core_video::pixel_buffer::{CVPixelBuffer, kCVPixelFormatType_32BGRA};
use std::{cell::Cell, rc::Rc, slice};

fn buffer() -> CVPixelBuffer {
    CVPixelBuffer::new(kCVPixelFormatType_32BGRA, 16, 16, None).expect("a 16x16 BGRA buffer")
}

struct Picture {
    buffer: CVPixelBuffer,
    builds: Rc<Cell<usize>>,
}

impl Render for Picture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.builds.set(self.builds.get() + 1);
        surface(self.buffer.clone()).size(px(64.))
    }
}

struct Tiles {
    picture: Entity<Picture>,
}

impl Render for Tiles {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .child(div().size(px(20.)).child("label"))
            .child(self.picture.clone())
    }
}

fn shown(cx: &mut TestAppContext, window: WindowHandle<Tiles>) -> Vec<CVPixelBuffer> {
    cx.update_window(window.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        window
            .rendered_frame
            .scene
            .surfaces
            .iter()
            .map(|surface| surface.image_buffer.clone())
            .collect()
    })
    .unwrap()
}

#[test]
fn a_surface_shows_the_buffer_its_view_holds_now() {
    let mut cx = TestAppContext::single();
    let builds = Rc::new(Cell::new(0));
    let first = buffer();
    let window = cx.add_window({
        let (first, builds) = (first.clone(), builds.clone());
        move |_, cx| Tiles {
            picture: cx.new(|_| Picture {
                buffer: first,
                builds,
            }),
        }
    });
    let picture = window
        .update(&mut cx, |tiles, _, _| tiles.picture.clone())
        .unwrap();
    assert_eq!(shown(&mut cx, window), slice::from_ref(&first));

    window.update(&mut cx, |_, _, cx| cx.notify()).unwrap();
    assert_eq!(
        shown(&mut cx, window),
        slice::from_ref(&first),
        "the picture is drawn from the last frame around its notified parent"
    );
    assert_eq!(builds.get(), 1);

    let second = buffer();
    picture.update(&mut cx, |picture, cx| {
        picture.buffer = second.clone();
        cx.notify();
    });
    assert_eq!(shown(&mut cx, window), slice::from_ref(&second));
    assert_eq!(builds.get(), 2);

    let third = buffer();
    picture.update(&mut cx, |picture, _| picture.buffer = third.clone());
    assert_eq!(
        shown(&mut cx, window),
        slice::from_ref(&third),
        "a buffer handed over without a notify is still the one shown"
    );

    window.update(&mut cx, |_, _, cx| cx.notify()).unwrap();
    assert_eq!(shown(&mut cx, window), [third]);
    assert_eq!(builds.get(), 3);
}
