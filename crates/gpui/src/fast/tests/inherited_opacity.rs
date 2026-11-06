//! Tests of what is drawn again from last frame under an opacity only paint
//! knows: a hover opacity on what holds it, which prepaint cannot see before
//! the frame's hit test. See `Window::note_painted_opacity`.

use crate::fast::layers::policy::Decision;
use crate::{
    AnyElement, AnyWindowHandle, App, AppContext as _, Context, Entity, InteractiveElement as _,
    IntoElement, ParentElement as _, Pixels, Point, Render, RetainedCheck, ScrollHandle,
    StatefulInteractiveElement as _, StyleRefinement, Styled as _, TestAppContext, Window,
    WindowHandle, div, point, px, rgb,
};

/// What the wrapper whose opacity changes holds.
#[derive(Clone, Copy, Debug)]
enum Held {
    /// A view, drawn again while nothing it read changed.
    View,
    /// A cached view ([`Entity::cached`]).
    Cached,
    /// A view holding a view, the one painted.
    Nested,
    /// A plain element, drawn again while it is built as it was.
    Element,
}

struct Leaf {
    renders: usize,
}

impl Render for Leaf {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        div().size(px(100.)).bg(rgb(0xff0000))
    }
}

struct Middle {
    leaf: Entity<Leaf>,
}

impl Render for Middle {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size(px(100.)).child(self.leaf.clone())
    }
}

struct Root {
    opacity: f32,
    width: Pixels,
    /// Whether the wrapper has an id, which keeps a hover state that its
    /// prepaint reads, where one without sees a hover only in paint.
    keyed: bool,
    held: Held,
    leaf: Entity<Leaf>,
    middle: Entity<Middle>,
}

impl Render for Root {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let held: AnyElement = match self.held {
            Held::View => self.leaf.clone().into_any_element(),
            Held::Cached => self
                .leaf
                .clone()
                .cached(StyleRefinement::default().size(px(100.)))
                .into_any_element(),
            Held::Nested => self.middle.clone().into_any_element(),
            Held::Element => div().size(px(100.)).bg(rgb(0xff0000)).into_any_element(),
        };
        let wrapper = div()
            .w(self.width)
            .h(px(200.))
            .opacity(self.opacity)
            .hover(|style| style.opacity(1.));
        if self.keyed {
            wrapper.id("wrapper").child(held).into_any_element()
        } else {
            wrapper.child(held).into_any_element()
        }
    }
}

struct Scene {
    window: WindowHandle<Root>,
    leaf: Entity<Leaf>,
}

impl Scene {
    fn new(cx: &mut TestAppContext, held: Held, keyed: bool, check: Option<RetainedCheck>) -> Self {
        let leaf = cx.new(|_| Leaf { renders: 0 });
        let middle = cx.new(|_| Middle { leaf: leaf.clone() });
        let window = cx.add_window({
            let leaf = leaf.clone();
            move |window, _| {
                window.set_check_retained(check);
                Root {
                    opacity: 1.,
                    width: px(200.),
                    keyed,
                    held,
                    leaf,
                    middle,
                }
            }
        });
        Scene { window, leaf }
    }

    /// Updates the root, telling it, which draws a frame, and draws the
    /// frames it asks for, as the platform delivers them: the painted alpha
    /// of the one quad, and how often the leaf rendered.
    fn update(
        &self,
        cx: &mut TestAppContext,
        update: impl FnOnce(&mut Root, &mut Window, &mut Context<Root>),
    ) -> (f32, usize) {
        self.window
            .update(cx, |root, window, cx| {
                update(root, window, cx);
                cx.notify();
            })
            .unwrap();
        for _ in 0..3 {
            // The next frame's callbacks tell what asked for it, and that
            // frame is drawn once they have run.
            let (alpha, asked) = cx
                .update_window(self.window.into(), |_, window, cx| {
                    let quads = &window.rendered_frame.scene.quads;
                    assert_eq!(quads.len(), 1, "one quad is painted");
                    let alpha = quads[0].background.solid.a;
                    (alpha, window.simulate_next_frame(cx))
                })
                .unwrap();
            if asked == 0 {
                return (alpha, self.leaf.read_with(cx, |leaf, _| leaf.renders));
            }
        }
        panic!("the window kept asking for frames");
    }
}

fn unchanged(_: &mut Root, _: &mut Window, _: &mut Context<Root>) {}

/// Whatever the wrapper holds, and whether or not its prepaint can see the
/// hover, what is painted under it follows its opacity: set, hovered by a
/// pointer moving in or out, and hovered by the wrapper growing under a
/// pointer that stays still, which only paint's hit test finds. Each is
/// checked against the same frame drawn from scratch as well.
#[test]
fn what_is_drawn_again_follows_the_opacity_it_is_painted_under() {
    for held in [Held::View, Held::Cached, Held::Nested, Held::Element] {
        for keyed in [false, true] {
            let cx = &mut TestAppContext::single();
            let scene = Scene::new(cx, held, keyed, Some(RetainedCheck::Panic));
            let case = format!("{held:?}, keyed: {keyed}");
            let alpha = |(alpha, _): (f32, usize)| alpha;
            let outside = point(px(500.), px(500.));
            let inside = point(px(50.), px(50.));
            let beside = point(px(250.), px(50.));
            let pointer_to = |to| {
                move |_: &mut Root, window: &mut Window, cx: &mut Context<Root>| {
                    window.simulate_mouse_move(to, cx)
                }
            };

            assert_eq!(alpha(scene.update(cx, pointer_to(outside))), 1., "{case}");
            assert_eq!(alpha(scene.update(cx, unchanged)), 1., "{case}");
            let fade = |root: &mut Root, _: &mut Window, _: &mut Context<Root>| root.opacity = 0.5;
            assert_eq!(alpha(scene.update(cx, fade)), 0.5, "{case}");
            assert_eq!(alpha(scene.update(cx, unchanged)), 0.5, "{case}");

            assert_eq!(
                alpha(scene.update(cx, pointer_to(inside))),
                1.,
                "{case}: hovered"
            );
            assert_eq!(alpha(scene.update(cx, unchanged)), 1., "{case}: hovered");
            assert_eq!(
                alpha(scene.update(cx, pointer_to(beside))),
                0.5,
                "{case}: left"
            );

            let widen =
                |root: &mut Root, _: &mut Window, _: &mut Context<Root>| root.width = px(300.);
            assert_eq!(alpha(scene.update(cx, widen)), 1., "{case}: grown under");
            assert_eq!(
                alpha(scene.update(cx, unchanged)),
                1.,
                "{case}: grown under"
            );
            let narrow =
                |root: &mut Root, _: &mut Window, _: &mut Context<Root>| root.width = px(200.);
            assert_eq!(alpha(scene.update(cx, narrow)), 0.5, "{case}: shrunk away");
        }
    }
}

/// A view is built again when the opacity it is painted under changes, and
/// only then: under a hover its prepaint never sees, which it foresees
/// wrongly the same way every frame, it is drawn again from last frame for
/// as long as the hover lasts.
#[test]
fn a_view_is_built_again_only_when_its_opacity_changes() {
    for held in [Held::View, Held::Cached, Held::Nested] {
        let cx = &mut TestAppContext::single();
        let scene = Scene::new(cx, held, false, None);
        let pointer_to = |to| {
            move |_: &mut Root, window: &mut Window, cx: &mut Context<Root>| {
                window.simulate_mouse_move(to, cx)
            }
        };
        let outside = pointer_to(point(px(500.), px(500.)));
        assert_eq!(scene.update(cx, outside), (1., 1), "{held:?}");
        assert_eq!(scene.update(cx, unchanged), (1., 1), "{held:?}");
        let fade = |root: &mut Root, _: &mut Window, _: &mut Context<Root>| root.opacity = 0.5;
        assert_eq!(scene.update(cx, fade), (0.5, 2), "{held:?}");
        assert_eq!(scene.update(cx, unchanged), (0.5, 2), "{held:?}");
        let inside = pointer_to(point(px(50.), px(50.)));
        assert_eq!(scene.update(cx, inside), (1., 3), "{held:?}: hovered");
        assert_eq!(scene.update(cx, unchanged), (1., 3), "{held:?}: hovered");
        let beside = pointer_to(point(px(250.), px(50.)));
        assert_eq!(scene.update(cx, beside), (0.5, 4), "{held:?}: left");
    }
}

/// Opacities are compared by their bits: under a NaN opacity, a view is
/// still drawn again from last frame.
#[test]
fn a_nan_opacity_still_draws_a_view_again() {
    for held in [Held::View, Held::Cached] {
        let cx = &mut TestAppContext::single();
        let scene = Scene::new(cx, held, false, None);
        let outside = point(px(500.), px(500.));
        let nan = move |root: &mut Root, window: &mut Window, cx: &mut Context<Root>| {
            window.simulate_mouse_move(outside, cx);
            root.opacity = f32::NAN;
        };
        let (alpha, renders) = scene.update(cx, nan);
        assert!(alpha.is_nan(), "{held:?}");
        assert_eq!(scene.update(cx, unchanged).1, renders, "{held:?}");
    }
}

/// A page whose scroll container sits in a wrapper with a hover opacity its
/// prepaint never sees, having no id.
struct Scrolling {
    handle: ScrollHandle,
}

impl Render for Scrolling {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().bg(rgb(0xffffff)).child(
            div()
                .w(px(200.))
                .h(px(100.))
                .opacity(0.5)
                .hover(|style| style.opacity(1.))
                .child(
                    div()
                        .id("scroller")
                        .overflow_y_scroll()
                        .track_scroll(&self.handle)
                        .size_full()
                        .children((0..40).map(|ix| div().h(px(20.)).bg(rgb(0x100000 + ix * 0x10)))),
                ),
        )
    }
}

/// A scroll container's layer composited on a frame that only scrolled it,
/// under another opacity than its content was painted at, is painted afresh
/// on the next frame; composited under the opacity it was painted at, it
/// stays composited, though prepaint foresees another.
#[test]
fn a_layer_composited_under_another_opacity_is_painted_again() {
    if !crate::fast::layers::COMPILED {
        return;
    }
    let cx = &mut TestAppContext::single();
    let handle = ScrollHandle::new();
    let window = cx.add_window({
        let handle = handle.clone();
        move |window, _| {
            window.set_check_retained(Some(RetainedCheck::Panic));
            Scrolling { handle }
        }
    });
    let window: AnyWindowHandle = window.into();
    let with_window = |cx: &mut TestAppContext, f: &mut dyn FnMut(&mut Window, &mut App)| {
        cx.update_window(window, |_, window, cx| f(window, cx))
            .unwrap()
    };
    // Scrolls to `y`, moving the pointer to `pointer` first, draws the frame
    // that follows and the frames it asks for, and returns what the container
    // decided in each.
    let scroll_to = |cx: &mut TestAppContext, y: f32, pointer: Point<Pixels>| {
        let frame = cx
            .update_window(window, |_, window, _| window.fast_layers.frame)
            .unwrap();
        with_window(cx, &mut |window, cx| {
            window.simulate_mouse_move(pointer, cx);
            handle.set_offset(point(px(0.), px(y)));
        });
        with_window(cx, &mut |window, cx| {
            if window.fast_layers.frame == frame {
                window.draw(cx).clear(cx);
            }
        });
        let mut decisions = Vec::new();
        for _ in 0..3 {
            let mut asked = 0;
            with_window(cx, &mut |window, cx| {
                let id = window
                    .fast_layers
                    .scrolls
                    .containers()
                    .find(|id| id.last() == Some(&"scroller".into()))
                    .cloned()
                    .expect("the scroll container was painted");
                decisions.push(crate::fast::layers::policy::last_decision(window, &id));
                asked = window.simulate_next_frame(cx);
            });
            if asked == 0 {
                return decisions;
            }
        }
        panic!("the window kept asking for frames");
    };
    let outside = point(px(500.), px(500.));
    let inside = point(px(50.), px(50.));
    scroll_to(cx, 0., outside);
    scroll_to(cx, -20., outside);
    assert_eq!(scroll_to(cx, -40., outside), [Some(Decision::Repaint)]);
    assert_eq!(scroll_to(cx, -60., outside), [Some(Decision::Composite)]);
    // Hovered, its content is painted at another opacity than it was.
    assert_eq!(
        scroll_to(cx, -80., inside),
        [Some(Decision::Composite), Some(Decision::Repaint)]
    );
    assert_eq!(scroll_to(cx, -100., inside), [Some(Decision::Composite)]);
    assert_eq!(
        scroll_to(cx, -120., outside),
        [Some(Decision::Composite), Some(Decision::Repaint)]
    );
}
