//! Tests of the retained check. See [`crate::fast::check_retained`].

use crate::{
    Animation, AnimationExt as _, AnyWindowHandle, App, AppContext as _, Context, Entity,
    IntoElement, ParentElement as _, Render, RetainedCheck, ScrollDelta, ScrollWheelEvent,
    Styled as _, TestAppContext, TouchPhase, Window, WindowHandle, div, point, px, rgb,
};
use std::{cell::Cell, ops::Range, rc::Rc, time::Duration};

/// A bar as wide as a number it shares with whoever holds the same cell,
/// which nothing tells it of.
struct Bar {
    width: Rc<Cell<f32>>,
}

impl Render for Bar {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(div().w(px(self.width.get())).h(px(4.)).bg(crate::black()))
    }
}

/// A page holding a bar, and a count it shows that it is notified of.
struct Page {
    bar: Entity<Bar>,
    count: u32,
}

impl Render for Page {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size(px(300.))
            .child(div().h(px(20.)).child(format!("count {}", self.count)))
            .child(div().h(px(20.)).child(self.bar.clone()))
    }
}

fn with_window<R>(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    f: impl FnOnce(&mut Window, &mut App) -> R,
) -> R {
    cx.update_window(window, |_, window, cx| f(window, cx))
        .unwrap()
}

fn draw(cx: &mut TestAppContext, window: AnyWindowHandle) {
    with_window(cx, window, |window, cx| window.draw(cx).clear(cx));
}

fn reports(cx: &mut TestAppContext, window: AnyWindowHandle) -> Vec<String> {
    with_window(cx, window, |window, _| window.take_retained_check_reports())
}

fn page(cx: &mut TestAppContext, check: RetainedCheck) -> (WindowHandle<Page>, Rc<Cell<f32>>) {
    let width = Rc::new(Cell::new(10.));
    let window = cx.add_window({
        let width = width.clone();
        move |window, cx| {
            window.set_check_retained(Some(check));
            Page {
                bar: cx.new(|_| Bar { width }),
                count: 0,
            }
        }
    });
    draw(cx, window.into());
    draw(cx, window.into());
    (window, width)
}

/// Counts the page's count up, which builds the page again and draws its
/// bar again from last frame.
fn count_up(cx: &mut TestAppContext, window: WindowHandle<Page>) {
    window
        .update(cx, |page, _, cx| {
            page.count += 1;
            cx.notify();
        })
        .unwrap();
    draw(cx, window.into());
}

/// A view whose state changed without it being notified, drawn again from
/// last frame, is caught on the frame that draws it again, named by its type
/// and its place; the window shows the frame it drew.
#[test]
fn a_view_drawn_again_without_being_notified_is_caught_and_named() {
    let mut cx = TestAppContext::single();
    let (window, width) = page(&mut cx, RetainedCheck::Record);
    count_up(&mut cx, window);
    assert_eq!(reports(&mut cx, window.into()), Vec::<String>::new());

    width.set(40.);
    count_up(&mut cx, window);
    let found = reports(&mut cx, window.into());
    assert!(!found.is_empty(), "the stale bar is caught");
    for report in &found {
        assert!(
            report.contains("check_retained::Bar") && report.contains("drawn from the last frame"),
            "{report}"
        );
        assert!(
            report.contains("drawn (1)") && report.contains("afresh (1)"),
            "{report}"
        );
    }
    // The check leaves the frame shown as the window drew it, stale bar and
    // all, as it would be with the check off.
    let shown = with_window(&mut cx, window.into(), |window, _| {
        window.painted_primitives().join("\n")
    });
    assert!(
        shown.contains("20px (scaled) × 8px") && !shown.contains("80px (scaled) × 8px"),
        "the bar is shown as drawn, 10 wide: {shown}"
    );

    // Told of it, the bar is built afresh, and nothing is caught.
    let bar = window
        .update(&mut cx, |page, _, _| page.bar.clone())
        .unwrap();
    bar.update(&mut cx, |_, cx| cx.notify());
    count_up(&mut cx, window);
    assert_eq!(reports(&mut cx, window.into()), Vec::<String>::new());
    let shown = with_window(&mut cx, window.into(), |window, _| {
        window.painted_primitives().join("\n")
    });
    assert!(
        shown.contains("80px (scaled) × 8px"),
        "the bar is shown 40 wide: {shown}"
    );
}

/// The same change told to the view is drawn afresh, and nothing is caught.
#[test]
fn a_view_notified_of_its_change_passes() {
    let mut cx = TestAppContext::single();
    let (window, width) = page(&mut cx, RetainedCheck::Panic);
    let bar = window
        .update(&mut cx, |page, _, _| page.bar.clone())
        .unwrap();
    for wide in [40., 60., 25.] {
        width.set(wide);
        bar.update(&mut cx, |_, cx| cx.notify());
        count_up(&mut cx, window);
    }
}

/// [`RetainedCheck::Panic`] panics with the report, as a test wants.
#[test]
#[should_panic(expected = "drawn from the last frame without being notified")]
fn the_check_panics_for_tests() {
    let mut cx = TestAppContext::single();
    let (window, width) = page(&mut cx, RetainedCheck::Panic);
    width.set(40.);
    count_up(&mut cx, window);
}

/// A view drawn again beside one that animates is judged at the moment the
/// animation stands at, and holds still: the check finds nothing.
#[test]
fn a_view_drawn_again_beside_an_animation_passes() {
    struct Fading {
        bar: Entity<Bar>,
    }
    impl Render for Fading {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .size(px(300.))
                .child(div().size(px(50.)).bg(crate::black()).with_animation(
                    "fade",
                    Animation::new(Duration::from_secs(10)),
                    |this, delta| this.opacity(delta),
                ))
                .child(div().h(px(20.)).child(self.bar.clone()))
        }
    }
    let mut cx = TestAppContext::single();
    let window = cx.add_window(|window, cx| {
        window.set_check_retained(Some(RetainedCheck::Panic));
        Fading {
            bar: cx.new(|_| Bar {
                width: Rc::new(Cell::new(10.)),
            }),
        }
    });
    for _ in 0..6 {
        cx.executor().advance_clock(Duration::from_millis(16));
        with_window(&mut cx, window.into(), |window, cx| {
            window.simulate_next_frame(cx);
        });
        draw(&mut cx, window.into());
    }
}

/// How tall the list is, in pixels.
const VIEWPORT_HEIGHT: f32 = 100.;
/// How wide the list is, in pixels.
const VIEWPORT_WIDTH: f32 = 200.;
/// How tall a row is, in pixels.
const ROW_HEIGHT: f32 = 20.;

/// A list whose rows are coloured by a shift nothing tells it of.
struct UntrackedList {
    shift: Rc<Cell<usize>>,
}

impl Render for UntrackedList {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let shift = self.shift.clone();
        div().size_full().bg(rgb(0xffffff)).child(
            crate::uniform_list("list", 1000, move |range: Range<usize>, _, _| {
                range
                    .map(|row| {
                        let row = row + shift.get();
                        div()
                            .w(px(VIEWPORT_WIDTH))
                            .h(px(ROW_HEIGHT))
                            .bg(crate::hsla((row % 97) as f32 / 97., 0.5, 0.5, 1.))
                    })
                    .collect::<Vec<_>>()
            })
            .w(px(VIEWPORT_WIDTH))
            .h(px(VIEWPORT_HEIGHT)),
        )
    }
}

fn wheel(cx: &mut TestAppContext, window: AnyWindowHandle, dy: f32) {
    let frame = with_window(cx, window, |window, _| window.fast_layers.frame);
    with_window(cx, window, |window, cx| {
        window.dispatch_event(
            crate::PlatformInput::ScrollWheel(ScrollWheelEvent {
                position: point(px(20.), px(20.)),
                delta: ScrollDelta::Pixels(point(px(0.), px(dy))),
                modifiers: Default::default(),
                touch_phase: TouchPhase::Moved,
                momentum_phase: None,
            }),
            cx,
        );
    });
    if with_window(cx, window, |window, _| window.fast_layers.frame) == frame {
        draw(cx, window);
    }
}

/// Whether the last frame composited the list's layer.
fn composited(cx: &mut TestAppContext, window: AnyWindowHandle) -> bool {
    with_window(cx, window, |window, _| {
        window.fast_layers.layers.keys().any(|id| {
            crate::fast::layers::policy::last_decision(window, id)
                == Some(crate::fast::layers::policy::Decision::Composite)
        })
    })
}

/// A list's layer composited while rows it holds read a change nothing told
/// it of is caught, named by the scroll container, while one told of it
/// passes; and with the check on, the layer still composites.
#[crate::test]
fn a_layer_composited_over_an_untold_change_is_caught(cx: &mut TestAppContext) {
    if !crate::fast::layers::COMPILED {
        return;
    }
    let shift = Rc::new(Cell::new(0));
    let window: AnyWindowHandle = cx
        .add_window({
            let shift = shift.clone();
            move |window, _| {
                window.set_check_retained(Some(RetainedCheck::Record));
                UntrackedList { shift }
            }
        })
        .into();
    draw(cx, window);
    draw(cx, window);
    let mut composites = 0;
    for dy in [-7., -9., -11., -6., -8., -5., -9., -7.] {
        wheel(cx, window, dy);
        composites += usize::from(composited(cx, window));
    }
    assert!(composites > 0, "the layer composites with the check on");
    assert_eq!(reports(cx, window), Vec::<String>::new());

    shift.set(5);
    let mut found = Vec::new();
    for dy in [-6., -8., -7.] {
        wheel(cx, window, dy);
        found.extend(reports(cx, window));
    }
    assert!(!found.is_empty(), "the stale layer is caught");
    assert!(
        found[0].contains("the scroll layer of") && found[0].contains("list"),
        "{}",
        found[0]
    );

    // Told of it, the layer is painted afresh and composites again cleanly.
    shift.set(9);
    with_window(cx, window, |window, _| window.refresh());
    draw(cx, window);
    for dy in [-6., -8., -7., -9.] {
        wheel(cx, window, dy);
    }
    assert_eq!(reports(cx, window), Vec::<String>::new());
}

/// The check leaves no trace: through random histories of edits, hovers,
/// focus moves, resizes, lists growing and scrolling, a window checking
/// every frame paints what a window that does not paints, frame for frame,
/// draws the same views from its last frame and does the same layout work,
/// with its accessibility tree alike; and the check finds nothing.
#[test]
fn a_checked_window_draws_every_frame_as_an_unchecked_one() {
    let checked: usize = (0..8)
        .map(|seed| super::oracle::run_checked(seed, 60, false))
        .sum();
    assert!(checked > 0, "the checked window never drew a view again");
    let checked: usize = (100..102)
        .map(|seed| super::oracle::run_checked(seed, 60, true))
        .sum();
    assert!(
        checked > 0,
        "with accessibility, it never drew a view again"
    );
}
