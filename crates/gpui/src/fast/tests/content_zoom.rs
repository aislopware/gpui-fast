//! Tests of a window's interface zoom (`fast/content_zoom.rs`): layout at a
//! smaller viewport, painting at a larger scale, and every coordinate that
//! crosses to the platform mapped.

use std::{cell::Cell, cell::RefCell, ops::Range, rc::Rc};

use crate::{
    Bounds, Context, ElementInputHandler, EntityInputHandler, FocusHandle,
    InteractiveElement as _, IntoElement, ListAlignment, ListState, Modifiers, ParentElement as _,
    Pixels, Point, Render, ScrollDelta, ScrollEvent, StatefulInteractiveElement as _, Styled as _,
    TestAppContext, UTF16Selection, VisualTestContext, Window, canvas,
    composition::{NativeHost, NativeHostOptions, native_view},
    div, list, point, px, rgb, size,
};

const ZOOM: f32 = 1.25;

fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
    });
}

fn zoom(cx: &mut VisualTestContext, zoom: f32) {
    cx.update(|window, cx| window.set_content_zoom(zoom, cx));
    draw(cx);
}

/// A button at (200, 200), 50 px on a side, that counts its clicks, over a
/// list of 20 px rows 100 px tall at the top left.
struct Page {
    clicks: Rc<Cell<usize>>,
    list: ListState,
}

impl Render for Page {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let clicks = self.clicks.clone();
        div()
            .relative()
            .size_full()
            .child(
                list(self.list.clone(), |_, _, _| {
                    div().h(px(20.)).w_full().into_any_element()
                })
                .absolute()
                .top_0()
                .left_0()
                .w(px(100.))
                .h(px(100.)),
            )
            .child(
                div()
                    .id("button")
                    .absolute()
                    .top(px(200.))
                    .left(px(200.))
                    .size(px(50.))
                    .bg(rgb(0x336699))
                    .on_click(move |_, _, _| clicks.set(clicks.get() + 1)),
            )
    }
}

fn page(cx: &mut TestAppContext) -> (Rc<Cell<usize>>, ListState, &mut VisualTestContext) {
    let clicks = Rc::new(Cell::new(0));
    let list = ListState::new(50, ListAlignment::Top, px(0.)).measure_all();
    let (_, cx) = cx.add_window_view({
        let (clicks, list) = (clicks.clone(), list.clone());
        move |_, _| Page { clicks, list }
    });
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    draw(cx);
    (clicks, list, cx)
}

/// The viewport shrinks and the scale grows by the zoom, so the drawable
/// keeps its size; the zoom is clamped, and setting it back restores both.
#[crate::test]
fn a_zoomed_window_lays_out_smaller_and_paints_larger(cx: &mut TestAppContext) {
    let (_, _, cx) = page(cx);
    let (viewport, scale) =
        cx.update(|window, _| (window.viewport_size(), window.scale_factor()));
    zoom(cx, ZOOM);
    let (zoomed_viewport, zoomed_scale, content_zoom) = cx.update(|window, _| {
        (
            window.viewport_size(),
            window.scale_factor(),
            window.content_zoom(),
        )
    });
    assert_eq!(content_zoom, ZOOM);
    assert_eq!(zoomed_viewport, viewport.map(|v| v / ZOOM));
    assert_eq!(zoomed_scale, scale * ZOOM);
    let device =
        |size: crate::Size<Pixels>, scale: f32| size.map(|v| (f32::from(v) * scale).round());
    assert_eq!(device(zoomed_viewport, zoomed_scale), device(viewport, scale));

    zoom(cx, 10.);
    assert_eq!(cx.update(|window, _| window.content_zoom()), 4.);
    zoom(cx, 1.);
    let restored = cx.update(|window, _| (window.viewport_size(), window.scale_factor()));
    assert_eq!(restored, (viewport, scale));
}

/// A click the platform reports at its own coordinates lands on what is
/// drawn there: the button at (200, 200)..(250, 250) shows at
/// (250, 250)..(312.5, 312.5) zoomed 1.25.
#[crate::test]
fn a_zoomed_window_hit_tests_the_platforms_points(cx: &mut TestAppContext) {
    let (clicks, _, cx) = page(cx);
    zoom(cx, ZOOM);

    cx.simulate_click(point(px(240.), px(240.)), Modifiers::none());
    assert_eq!(clicks.get(), 0, "(192, 192) in the window: beside the button");
    cx.simulate_click(point(px(260.), px(260.)), Modifiers::none());
    assert_eq!(clicks.get(), 1, "(208, 208) in the window: on the button");
    cx.simulate_click(point(px(310.), px(310.)), Modifiers::none());
    assert_eq!(clicks.get(), 2, "(248, 248): still on it");

    cx.simulate_mouse_move(point(px(100.), px(50.)), None, Modifiers::none());
    let position = cx.update(|window, _| window.mouse_position());
    assert_eq!(position, point(px(80.), px(40.)));
}

/// A trackpad's pixels scroll the window's pixels: 50 of the platform's are
/// 40 of a window zoomed 1.25.
#[crate::test]
fn a_zoomed_window_scrolls_by_its_own_pixels(cx: &mut TestAppContext) {
    let (_, list, cx) = page(cx);
    zoom(cx, ZOOM);
    cx.simulate_event(ScrollEvent {
        position: point(px(50.), px(50.)),
        delta: ScrollDelta::Pixels(point(px(0.), px(-50.))),
        ..Default::default()
    });
    let top = list.logical_scroll_top();
    assert_eq!((top.item_ix, top.offset_in_item), (2, px(0.)));
}

/// Asking for a content size asks the platform for it zoomed, and the
/// viewport it reports back is the size asked for.
#[crate::test]
fn a_zoomed_window_resizes_by_its_own_size(cx: &mut TestAppContext) {
    let (_, _, cx) = page(cx);
    zoom(cx, ZOOM);
    let (platform, viewport) = cx.update(|window, cx| {
        window.resize(size(px(400.), px(300.)));
        window.bounds_changed(cx);
        (window.platform_window.content_size(), window.viewport_size())
    });
    assert_eq!(platform, size(px(500.), px(375.)));
    assert_eq!(viewport, size(px(400.), px(300.)));
}

/// A text field whose caret is at (10, 20), 2 × 16 px, and which notes the
/// points it is asked a character for.
struct Field {
    focus_handle: FocusHandle,
    asked: Rc<RefCell<Vec<Point<Pixels>>>>,
}

const CARET: Bounds<Pixels> = Bounds {
    origin: Point {
        x: px(10.),
        y: px(20.),
    },
    size: crate::Size {
        width: px(2.),
        height: px(16.),
    },
};

impl Render for Field {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity();
        let focus_handle = self.focus_handle.clone();
        div().size_full().track_focus(&self.focus_handle).child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, cx| {
                    window.handle_input(&focus_handle, ElementInputHandler::new(bounds, view), cx);
                },
            )
            .size_full(),
        )
    }
}

impl EntityInputHandler for Field {
    fn text_for_range(
        &mut self,
        _range: Range<usize>,
        _adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        None
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}

    fn replace_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        _text: &str,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        _new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        Some(CARET)
    }

    fn selection_bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Vec<Bounds<Pixels>> {
        vec![CARET, CARET.map_origin(|v| v + px(20.))]
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        self.asked.borrow_mut().push(point);
        Some(0)
    }
}

/// The rectangles an IME places its candidates by are the platform's, the
/// window's times the zoom, and the point it asks a character at is the
/// window's, the platform's divided by it.
#[crate::test]
fn a_zoomed_windows_ime_rects_are_the_platforms(cx: &mut TestAppContext) {
    let asked = Rc::new(RefCell::new(Vec::new()));
    let (field, cx) = cx.add_window_view({
        let asked = asked.clone();
        move |_, cx| Field {
            focus_handle: cx.focus_handle(),
            asked,
        }
    });
    let focus = field.read_with(cx, |field, _| field.focus_handle.clone());
    cx.update(|window, cx| window.focus(&focus, cx));
    zoom(cx, ZOOM);

    let mut handler = cx
        .update(|window, _| window.platform_window.take_input_handler())
        .expect("the field's input handler");
    let zoomed = CARET.map(|v| v * ZOOM);
    assert_eq!(handler.bounds_for_range(0..0), Some(zoomed));
    assert_eq!(handler.ime_candidate_bounds(), Some(zoomed));
    assert_eq!(
        handler.selection_bounds_for_range(0..1),
        vec![zoomed, CARET.map_origin(|v| v + px(20.)).map(|v| v * ZOOM)]
    );
    assert_eq!(
        handler.character_index_for_point(point(px(125.), px(250.))),
        Some(0)
    );
    assert_eq!(*asked.borrow(), vec![point(px(100.), px(200.))]);
    let selected = cx.update(|window, cx| handler.selected_bounds(window, cx));
    assert_eq!(selected, Some(zoomed));
    cx.update(|window, _| window.platform_window.set_input_handler(handler));
}

/// A native at (10, 10), 100 px on a side.
struct Native {
    host: NativeHost,
}

impl Render for Native {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().relative().size_full().child(
            native_view(&self.host)
                .absolute()
                .top(px(10.))
                .left(px(10.))
                .size(px(100.)),
        )
    }
}

/// A native is placed, and takes the pointer, in the platform's points: at
/// (12.5, 12.5)..(137.5, 137.5) for a window zoomed 1.25.
#[crate::test]
fn a_zoomed_window_places_natives_in_the_platforms_points(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|window, cx| Native {
        host: window
            .create_native_host(NativeHostOptions::default(), cx)
            .expect("the test platform composes natives"),
    });
    cx.update(|window, cx| window.set_content_zoom(ZOOM, cx));
    cx.update(|window, cx| {
        window.refresh();
        window.draw_and_present(cx);
    });
    let (placement, hit_map) = cx.update(|window, _| {
        let natives = window.presented_natives().expect("a present");
        (natives.placements[0].clone(), window.native_hit_map().unwrap())
    });
    let platform = Bounds::new(point(px(12.5), px(12.5)), size(px(125.), px(125.)));
    assert_eq!(placement.bounds, platform);
    let id = placement.id;
    assert_eq!(hit_map.native_at(point(px(130.), px(130.))), Some(id));
    assert_eq!(hit_map.native_at(point(px(11.), px(11.))), None);
}
