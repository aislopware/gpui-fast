//! Elements drawn again moved must paint what painting them afresh paints.
//! See [`crate::fast::shift`].
//!
//! Each test drives a window drawing incrementally and one forgetting what
//! it retains before every frame through the same moves, and requires every
//! frame to match.

use std::sync::Arc;

use super::element_oracle::GlyphBoxTextSystem;
use crate::{
    Context, Div, Entity, IntoElement, LayoutStats, NoopTextSystem, Render, ScrollHandle,
    SharedString, TestAppContext, Window, WindowHandle, div, hsla, point, prelude::*, px,
};

/// A clipped box at the window's top holding rows scrolled past its top, a
/// column of plain rows below a margin, which may be negative, and a
/// clipped section that moves with its clip.
struct Moving {
    top: f32,
    scroll: ScrollHandle,
    spacer: f32,
    margin: f32,
}

impl Render for Moving {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div().mt(px(self.top)).h(px(40.)).overflow_hidden().child(
                    div()
                        .id("scrolled")
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll)
                        .h(px(40.))
                        .children((0..4).map(|row| {
                            div()
                                .h(px(20.))
                                .bg(hsla(row as f32 / 4., 0.5, 0.5, 1.))
                                .child(format!("scrolled {row}"))
                        })),
                ),
            )
            .child(div().h(px(40.)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .mt(px(self.spacer))
                    .children((0..6).map(|row| {
                        div()
                            .flex()
                            .flex_row()
                            .gap_1()
                            .h(px(20.))
                            .bg(hsla(row as f32 / 6., 0.5, 0.5, 1.))
                            .border_b_1()
                            .border_color(hsla(0., 0., 0., 1.))
                            .child(format!("row {row}"))
                            .child(div().size(px(6.)).bg(hsla(0.3, 0.5, 0.5, 1.)))
                    })),
            )
            .child(
                div()
                    .ml(px(self.margin))
                    .w(px(120.))
                    .h(px(30.))
                    .overflow_hidden()
                    .child(div().w(px(300.)).child("clipped and moved with its clip")),
            )
    }
}

/// In a clip that stands still, a box larger than it that clips a label,
/// below a spacer: moved by the spacer, it is drawn again moved with what
/// it clips.
struct Curtain {
    spacer: f32,
}

impl Render for Curtain {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            div().mt(px(100.)).h(px(100.)).overflow_hidden().child(
                div()
                    .flex()
                    .flex_col()
                    .child(div().flex_none().h(px(self.spacer)))
                    .child(
                        div()
                            .flex_none()
                            .mt(px(-60.))
                            .size(px(3000.))
                            .overflow_hidden()
                            .child(div().mt(px(70.)).ml(px(10.)).child("curtain")),
                    ),
            ),
        )
    }
}

/// A clipped box at the window's left edge holding a line scrolled past
/// it, the box moving right with the line.
struct Sideways {
    left: f32,
    scroll: ScrollHandle,
}

impl Render for Sideways {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            div().ml(px(self.left)).w(px(200.)).overflow_hidden().child(
                div()
                    .id("sideways")
                    .overflow_x_scroll()
                    .track_scroll(&self.scroll)
                    .w(px(200.))
                    .h(px(20.))
                    .child(div().flex_none().w(px(400.)).child("ab ab")),
            ),
        )
    }
}

/// In a clip a margin from the window's left edge, a count and a narrow
/// box, its line of text aligned right and overhanging it to the left, its
/// first glyph less than a pixel left of the clip at the window's edge.
struct Overhang {
    left: f32,
    count: usize,
}

impl Render for Overhang {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let text = SharedString::from("abcdefgh");
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let width = window
            .text_system()
            .shape_line(text.clone(), font_size, &[style.to_run(text.len())], None)
            .width;
        div().size_full().child(
            div()
                .ml(px(self.left))
                .w(px(200.))
                .h(px(80.))
                .overflow_hidden()
                .child(self.count.to_string())
                .child(
                    div()
                        .ml(px((width.0 - 30.).floor()))
                        .w(px(30.))
                        .whitespace_nowrap()
                        .text_right()
                        .child(text),
                ),
        )
    }
}

/// In a clip that stands still, a section below a spacer holding a label
/// in a clip of its own, and a box that clips nothing.
struct Pane {
    spacer: f32,
}

impl Render for Pane {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            div().mt(px(100.)).h(px(100.)).overflow_hidden().child(
                div()
                    .flex()
                    .flex_col()
                    .child(div().flex_none().h(px(self.spacer)))
                    .child(
                        div()
                            .flex_none()
                            .h(px(40.))
                            .bg(hsla(0.6, 0.5, 0.5, 1.))
                            .child(
                                div()
                                    .ml(px(10.))
                                    .w(px(50.))
                                    .h(px(20.))
                                    .overflow_hidden()
                                    .child(div().w(px(300.)).child("clipped by its own clip")),
                            )
                            .child(div().overflow_hidden().child("clipping nothing")),
                    ),
            ),
        )
    }
}

/// In a clip that stands still, a row of boxes after a margin, which may
/// be negative, sliding sideways through it.
struct Slide {
    margin: f32,
}

impl Render for Slide {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            div()
                .ml(px(100.))
                .w(px(100.))
                .h(px(20.))
                .overflow_hidden()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .ml(px(self.margin))
                        .w(px(400.))
                        .children((0..8).map(|index| {
                            div().flex_none().w(px(30.)).h(px(20.)).bg(hsla(
                                index as f32 / 8.,
                                0.5,
                                0.5,
                                1.,
                            ))
                        })),
                ),
        )
    }
}

/// Below a header, rows under a scroll, each bordered around corners of a
/// fractional radius, whose sides can come out a rounding apart moved, and
/// so are never drawn again moved; drawn by views of their own, if `views`.
struct Rounded {
    scroll: f32,
    views: Option<Vec<Entity<RoundedRow>>>,
}

struct RoundedRow(usize);

fn rounded_row(row: usize) -> Div {
    div()
        .flex_none()
        .h(px(20.))
        .border_1()
        .border_color(hsla(0., 0., 0., 1.))
        .rounded(px(2.3))
        .child(format!("row {row}"))
}

impl Render for RoundedRow {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        rounded_row(self.0)
    }
}

impl Render for Rounded {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let rows = div().flex().flex_col();
        let rows = match &self.views {
            Some(views) => rows.children(views.iter().cloned()),
            None => rows.children((0..8).map(rounded_row)),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(div().flex_none().h(px(20.)).child("header"))
            .child(div().flex_none().h(px(self.scroll)))
            .child(rows)
    }
}

impl Moving {
    fn new() -> Self {
        let scroll = ScrollHandle::new();
        scroll.set_offset(point(px(0.), px(-25.)));
        Moving {
            top: 0.,
            scroll,
            spacer: 0.,
            margin: 0.,
        }
    }
}

/// Opens the two windows, drawing what `new` makes.
fn windows<V: Render>(cx: &mut TestAppContext, new: impl Fn() -> V) -> [WindowHandle<V>; 2] {
    let incremental = cx.add_window(|_, _| new());
    let from_scratch = cx.add_window(|_, _| new());
    let atlas = cx
        .update_window(incremental.into(), |_, window, _| {
            window.sprite_atlas.clone()
        })
        .unwrap();
    cx.update_window(from_scratch.into(), |_, window, _| {
        window.sprite_atlas = atlas;
    })
    .unwrap();
    [incremental, from_scratch]
}

/// Changes both windows' views as `step` says, which draws them, draws the
/// other one again from scratch, requires the frames to match and returns
/// the incremental window's stats since the last step.
fn step<V: Render>(
    cx: &mut TestAppContext,
    [incremental, from_scratch]: [WindowHandle<V>; 2],
    step: impl Fn(&mut V),
) -> LayoutStats {
    for window in [incremental, from_scratch] {
        window
            .update(cx, |view, _, cx| {
                step(view);
                cx.notify();
            })
            .unwrap();
    }
    let draw = |cx: &mut TestAppContext, window: WindowHandle<V>, forget: bool| {
        cx.update_window(window.into(), |_, window, cx| {
            if forget {
                window.forget_retained_state();
                window.draw(cx).clear(cx);
            }
            let stats = window.layout_stats();
            window.reset_layout_stats();
            (window.describe_rendered_frame(), stats)
        })
        .unwrap()
    };
    let (expected, _) = draw(cx, from_scratch, true);
    let (actual, stats) = draw(cx, incremental, false);
    assert_eq!(actual, expected);
    stats
}

fn text_cx() -> TestAppContext {
    TestAppContext::with_text_system(Arc::new(GlyphBoxTextSystem(NoopTextSystem)))
}

#[test]
fn rows_moved_by_whole_device_pixels_are_drawn_again_moved() {
    let mut cx = text_cx();
    let windows = windows(&mut cx, Moving::new);
    let mut moved = 0;
    // Half a pixel a frame, a device pixel at a scale factor of 2.
    for _ in 0..12 {
        moved += step(&mut cx, windows, |moving| moving.spacer += 0.5).elements_moved;
    }
    assert!(moved > 0, "nothing was drawn again moved");
}

/// At a scale factor of 1.5, a move by a whole logical pixel, all layout
/// gives, is a move by a device pixel and a half, which snapping and glyph
/// steps can't follow: those are painted afresh, and moves by two logical
/// pixels are drawn again moved.
#[test]
fn a_move_by_part_of_a_device_pixel_is_painted_afresh() {
    let mut cx = text_cx();
    let windows = windows(&mut cx, Moving::new);
    for window in windows {
        cx.simulate_window_scale_factor_change(window.into(), 1.5);
    }
    step(&mut cx, windows, |_| {});
    for _ in 0..8 {
        let stats = step(&mut cx, windows, |moving| moving.spacer += 1.);
        assert_eq!(stats.elements_moved, 0, "{stats:?}");
    }
    let mut moved = 0;
    for _ in 0..8 {
        moved += step(&mut cx, windows, |moving| moving.spacer += 2.).elements_moved;
    }
    assert!(moved > 0, "nothing was drawn again moved");
}

#[test]
fn a_section_moved_with_its_clip_is_drawn_again_moved() {
    let mut cx = text_cx();
    let windows = windows(&mut cx, Moving::new);
    let mut moved = 0;
    for _ in 0..8 {
        moved += step(&mut cx, windows, |moving| moving.margin += 3.).elements_moved;
    }
    assert!(moved > 0, "nothing was drawn again moved");
}

/// The box clips its label as the clip around it does until its top edge
/// comes into that clip, from where the label's mask is the box's: moving
/// the box with its label can't tell which the label's mask is, and paints
/// them afresh.
#[test]
fn a_clip_larger_than_the_one_around_it_moving_into_it_is_painted_afresh() {
    let mut cx = text_cx();
    let windows = windows(&mut cx, || Curtain { spacer: 30. });
    for _ in 0..3 {
        step(&mut cx, windows, |_| {});
    }
    for _ in 0..10 {
        step(&mut cx, windows, |curtain| curtain.spacer += 10.);
    }
}

/// Rows moving up past the window's top edge round there as they would
/// drawn afresh.
#[test]
fn rows_moving_past_the_top_edge_match() {
    let mut cx = text_cx();
    let windows = windows(&mut cx, Moving::new);
    for _ in 0..40 {
        step(&mut cx, windows, |moving| moving.spacer -= 1.5);
    }
}

/// A row scrolled past the top of a clip at the window's top, which moves
/// down with it: what lies above the window is not moved, for coordinates
/// rounded toward zero there would round the other way once moved below it.
#[test]
fn rows_scrolled_past_the_top_of_a_moving_clip_match() {
    let mut cx = text_cx();
    let windows = windows(&mut cx, Moving::new);
    // Drawn where they are first, for them to be moved from there.
    for _ in 0..3 {
        step(&mut cx, windows, |_| {});
    }
    for _ in 0..10 {
        step(&mut cx, windows, |moving| moving.top += 5.);
    }
}

/// A line scrolled past the left edge of a clip at the window's left edge,
/// which moves right with it: its second glyph lies a fraction of a device
/// pixel left of the window, where its place truncates toward zero, and
/// right of it once moved, where it truncates the other way. What lies left
/// of the window is not moved, and is painted afresh.
#[test]
fn a_line_scrolled_past_the_left_of_a_moving_clip_matches() {
    let mut cx = text_cx();
    let windows = windows(&mut cx, || {
        let scroll = ScrollHandle::new();
        scroll.set_offset(point(px(-10.), px(0.)));
        Sideways { left: 0., scroll }
    });
    for _ in 0..3 {
        step(&mut cx, windows, |_| {});
    }
    for _ in 0..6 {
        step(&mut cx, windows, |sideways| sideways.left += 5.);
    }
}

/// The overhanging line's first glyph, less than a device pixel left of the
/// window, is placed by truncating toward zero, and once moved right past
/// the window's edge, the other way. Its box lies right of the edge, and the
/// clip it is drawn in moves with it, but what places a glyph left of the
/// edge is painted afresh.
#[test]
fn a_glyph_left_of_the_window_moving_past_its_edge_matches() {
    let mut cx = text_cx();
    let windows = windows(&mut cx, || Overhang { left: 0., count: 0 });
    for window in windows {
        cx.simulate_window_scale_factor_change(window.into(), 1.);
    }
    for _ in 0..3 {
        step(&mut cx, windows, |_| {});
    }
    let mut moved = 0;
    for _ in 0..8 {
        let stats = step(&mut cx, windows, |overhang| {
            overhang.left += 1.;
            overhang.count += 1;
        });
        moved += stats.elements_moved;
    }
    assert!(moved > 0, "nothing was drawn again moved");
}

/// A section moving inside a clip that stands still, holding a clip of its
/// own, is drawn again moved with the new clip around it while its own lies
/// inside that, and painted afresh where it reaches the clip around's edge,
/// or comes back into it from past it.
#[test]
fn a_section_holding_a_clip_moved_inside_a_still_one_matches() {
    let mut cx = text_cx();
    let windows = windows(&mut cx, || Pane { spacer: 0. });
    for _ in 0..3 {
        step(&mut cx, windows, |_| {});
    }
    let mut moved = 0;
    for _ in 0..240 {
        moved += step(&mut cx, windows, |pane| pane.spacer += 0.5).elements_moved;
    }
    // Back up into the clip from past its bottom edge.
    for _ in 0..240 {
        moved += step(&mut cx, windows, |pane| pane.spacer -= 0.5).elements_moved;
    }
    assert!(moved > 0, "nothing was drawn again moved");
}

/// Rows that move every frame but can't be drawn again moved come to
/// nothing, and rest for it like rows built anew every frame: they are not
/// recorded every frame for a move that never comes.
fn rows_that_cannot_be_moved_rest(cx: &mut TestAppContext, views: bool) {
    let windows = windows(cx, || Rounded {
        scroll: 0.,
        views: None,
    });
    if views {
        for window in windows {
            window
                .update(cx, |rounded, _, cx| {
                    rounded.views = Some((0..8).map(|row| cx.new(|_| RoundedRow(row))).collect());
                })
                .unwrap();
        }
    }
    for _ in 0..8 {
        step(cx, windows, |rows| rows.scroll += 0.5);
    }
    let mut built = 0;
    let frames = 32;
    for _ in 0..frames {
        let stats = step(cx, windows, |rows| rows.scroll += 0.5);
        assert_eq!(stats.elements_moved, 0, "{stats:?}");
        built += stats.elements_built;
    }
    // The window's elements, recorded on a frame in four at most.
    let elements = 5 + 8 * 2;
    assert!(built <= elements * frames / 4, "{built} built");
}

#[test]
fn rows_that_cannot_be_moved_rest_as_elements() {
    rows_that_cannot_be_moved_rest(&mut text_cx(), false);
}

#[test]
fn rows_that_cannot_be_moved_rest_as_views() {
    rows_that_cannot_be_moved_rest(&mut text_cx(), true);
}

/// Boxes left out beyond either side of a clip that stands still come back
/// into it as the row slides one way, and move away from it the other way:
/// moved away they are drawn again moved, moved back they are painted
/// afresh.
#[test]
fn boxes_sliding_through_a_still_clip_match() {
    let mut cx = text_cx();
    let windows = windows(&mut cx, || Slide { margin: -120. });
    for _ in 0..3 {
        step(&mut cx, windows, |_| {});
    }
    let mut moved = 0;
    for _ in 0..40 {
        moved += step(&mut cx, windows, |slide| slide.margin += 5.).elements_moved;
    }
    for _ in 0..40 {
        moved += step(&mut cx, windows, |slide| slide.margin -= 5.).elements_moved;
    }
    assert!(moved > 0, "nothing was drawn again moved");
}

/// In a clip that stands still, a line longer than the clip after a margin,
/// sliding left through it.
struct Ticker {
    margin: f32,
}

impl Render for Ticker {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().child(
            div()
                .ml(px(100.))
                .w(px(100.))
                .h(px(20.))
                .overflow_hidden()
                .child(
                    div().flex().flex_row().ml(px(self.margin)).child(
                        div()
                            .flex_none()
                            .w(px(600.))
                            .child("a line far longer than the clip it slides through"),
                    ),
                ),
        )
    }
}

/// Glyphs past the right edge of a clip that stands still are not painted,
/// and come into it as the line slides left. The line's layer reaches the
/// clip's edge, so the line is painted afresh rather than drawn again moved
/// without them.
#[test]
fn a_line_sliding_into_a_still_clip_matches() {
    let mut cx = text_cx();
    let windows = windows(&mut cx, || Ticker { margin: 60. });
    for _ in 0..3 {
        step(&mut cx, windows, |_| {});
    }
    for _ in 0..12 {
        step(&mut cx, windows, |ticker| ticker.margin -= 5.);
    }
}

/// A box whose clip is switched on and off, holding a card that lies past
/// its right edge, with a panel inside the card.
struct Clipped {
    clipped: bool,
}

impl Render for Clipped {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let root = div()
            .relative()
            .w(px(100.))
            .h(px(100.))
            .bg(hsla(0.3, 0.3, 0.6, 1.));
        let root = if self.clipped {
            root.overflow_hidden()
        } else {
            root
        };
        root.child(
            div()
                .absolute()
                .left(px(150.))
                .top(px(10.))
                .w(px(60.))
                .h(px(60.))
                .bg(hsla(0.6, 0.5, 0.5, 1.))
                .child(
                    div()
                        .m(px(4.))
                        .w(px(40.))
                        .h(px(40.))
                        .border_1()
                        .border_color(hsla(0.1, 0.8, 0.5, 1.)),
                ),
        )
    }
}

/// A card the clip around it left out entirely, painted before anything
/// moved and so noting nothing, comes back when the clip is switched off:
/// it stood still, but the mask around it grew, and what it left out is
/// not known to lie outside the new one.
#[test]
fn what_a_clip_left_out_comes_back_when_the_clip_is_switched_off() {
    let mut cx = text_cx();
    let windows = windows(&mut cx, || Clipped { clipped: true });
    for _ in 0..2 {
        step(&mut cx, windows, |_| {});
    }
    for _ in 0..4 {
        step(&mut cx, windows, |view| view.clipped = !view.clipped);
    }
}

/// A bar centred in a box, holding a label taller than it, centred too,
/// which starts half a device pixel past a pixel's edge, where layout snaps
/// it toward zero; drawn by a view of its own, if `view`.
struct Straddle {
    height: f32,
    view: Option<Entity<StraddleBar>>,
}

struct StraddleBar;

fn straddle_bar() -> Div {
    div()
        .flex_none()
        .h(px(8.))
        .flex()
        .items_center()
        .bg(hsla(0.6, 0.5, 0.5, 1.))
        .child(
            div()
                .flex_none()
                .w(px(40.))
                .h(px(9.5))
                .bg(hsla(0.1, 0.8, 0.5, 1.))
                .child("label"),
        )
}

impl Render for StraddleBar {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        straddle_bar()
    }
}

impl Render for Straddle {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let column = div()
            .flex_none()
            .h(px(self.height))
            .flex()
            .flex_col()
            .justify_center();
        let column = match &self.view {
            Some(view) => column.child(view.clone()),
            None => column.child(straddle_bar()),
        };
        div().size_full().child(column)
    }
}

/// Grows the box by a device pixel at a time, which moves the bar it
/// centres by half of one, a move its bounds, snapped, do not always show:
/// the label it centres, half a device pixel off a pixel's edge, then snaps
/// a whole device pixel lower or higher, and drawn again where it was, would
/// stay a pixel off where painting it afresh puts it.
fn straddle(cx: &mut TestAppContext, windows: [WindowHandle<Straddle>; 2]) {
    let mut reused = 0;
    for _ in 0..3 {
        let stats = step(cx, windows, |_| {});
        reused += stats.elements_reused + stats.views_reused;
    }
    assert!(reused > 0, "nothing was drawn again");
    for _ in 0..4 {
        step(cx, windows, |straddle| straddle.height += 0.5);
    }
}

#[test]
fn a_move_its_snapped_bounds_hide_is_painted_afresh() {
    let mut cx = text_cx();
    let windows = windows(&mut cx, || Straddle {
        height: 20.,
        view: None,
    });
    straddle(&mut cx, windows);
}

#[test]
fn a_view_moved_by_part_of_a_pixel_its_bounds_hide_is_drawn_afresh() {
    let mut cx = text_cx();
    let windows = windows(&mut cx, || Straddle {
        height: 20.,
        view: None,
    });
    for window in windows {
        window
            .update(&mut cx, |straddle, _, cx| {
                straddle.view = Some(cx.new(|_| StraddleBar));
            })
            .unwrap();
    }
    straddle(&mut cx, windows);
}
