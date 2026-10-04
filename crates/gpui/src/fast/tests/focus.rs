//! Views that read the focus are built again when what they read of it
//! changes, and only then. See [`crate::fast::focus`].
//!
//! Each test drives a window drawing incrementally and one forgetting what
//! it retains before every frame through the same focus moves, and requires
//! every frame to match.

use crate::{
    AppContext as _, Context, Entity, FocusHandle, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, Styled as _, TestAppContext, Window, WindowHandle, div, hsla, px,
};
use std::{cell::Cell, rc::Rc};

/// A focusable box, blue while focused, labelled by whether it is, counting
/// its builds.
struct Tile {
    focus: FocusHandle,
    builds: Rc<Cell<usize>>,
}

impl Render for Tile {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.builds.set(self.builds.get() + 1);
        let focused = self.focus.is_focused(window);
        div()
            .track_focus(&self.focus)
            .w(px(40.))
            .h(px(20.))
            .bg(hsla(0., 0., 0.5, 1.))
            .focus(|style| style.bg(hsla(0.6, 0.8, 0.5, 1.)))
            .child(if focused { "on" } else { "off" })
    }
}

/// A panel holding two tiles, outlined while the focus is inside it.
struct Panel {
    focus: FocusHandle,
    tiles: [Entity<Tile>; 2],
    builds: Rc<Cell<usize>>,
}

impl Render for Panel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.builds.set(self.builds.get() + 1);
        let inside = self.focus.contains_focused(window, cx);
        div()
            .track_focus(&self.focus)
            .flex()
            .gap_1()
            .p_1()
            .border_1()
            .border_color(if inside {
                hsla(0.3, 0.8, 0.5, 1.)
            } else {
                hsla(0., 0., 0., 1.)
            })
            .children(self.tiles.clone())
    }
}

/// A status line naming whether anything has the focus, read through
/// [`Window::focused`].
struct Status {
    builds: Rc<Cell<usize>>,
}

impl Render for Status {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.builds.set(self.builds.get() + 1);
        let focused = window.focused(cx).is_some();
        div()
            .h(px(20.))
            .child(if focused { "focused" } else { "idle" })
    }
}

/// Three tiles, the first two in a panel, and a status line.
struct Screen {
    panel: Entity<Panel>,
    third: Entity<Tile>,
    status: Entity<Status>,
}

impl Render for Screen {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(self.panel.clone())
            .child(self.third.clone())
            .child(self.status.clone())
    }
}

/// How often each view of a window was built.
#[derive(Clone, Default)]
struct Builds {
    tiles: [Rc<Cell<usize>>; 3],
    panel: Rc<Cell<usize>>,
    status: Rc<Cell<usize>>,
}

impl Builds {
    fn get(&self) -> [usize; 5] {
        [
            self.tiles[0].get(),
            self.tiles[1].get(),
            self.tiles[2].get(),
            self.panel.get(),
            self.status.get(),
        ]
    }
}

fn open(cx: &mut TestAppContext, builds: &Builds) -> WindowHandle<Screen> {
    let builds = builds.clone();
    cx.add_window(move |_, cx| {
        let tile = |cx: &mut Context<Screen>, builds: &Rc<Cell<usize>>| {
            let builds = builds.clone();
            cx.new(|cx| Tile {
                focus: cx.focus_handle(),
                builds,
            })
        };
        let tiles = [tile(cx, &builds.tiles[0]), tile(cx, &builds.tiles[1])];
        let third = tile(cx, &builds.tiles[2]);
        let panel_builds = builds.panel.clone();
        let panel = cx.new(|cx| Panel {
            focus: cx.focus_handle(),
            tiles,
            builds: panel_builds,
        });
        let status = cx.new(|_| Status {
            builds: builds.status,
        });
        Screen {
            panel,
            third,
            status,
        }
    })
}

/// The focus handles of a window's tiles, first to third.
fn tiles(cx: &mut TestAppContext, window: WindowHandle<Screen>) -> [FocusHandle; 3] {
    window
        .update(cx, |screen, _, cx| {
            let [first, second] = &screen.panel.read(cx).tiles;
            [first, second, &screen.third].map(|tile| tile.read(cx).focus.clone())
        })
        .unwrap()
}

/// Moves both windows' focus to their `tile`, or blurs them, if `to` says
/// to, draws them, the second from scratch, requires the frames to match and
/// returns how often each view of the first was built for it: its tiles, its
/// panel, and whether its status line was. The status line reads the focus
/// of every window as a whole, so the second window's move builds it too.
fn step(
    cx: &mut TestAppContext,
    windows: [WindowHandle<Screen>; 2],
    builds: &Builds,
    to: Option<Option<usize>>,
) -> ([usize; 4], bool) {
    // Moving the focus draws the window as its update ends.
    let before = builds.get();
    for window in windows {
        let Some(tile) = to else { break };
        let handles = tiles(cx, window);
        cx.update_window(window.into(), |_, window, cx| match tile {
            Some(tile) => window.focus(&handles[tile], cx),
            None => window.blur(cx),
        })
        .unwrap();
    }
    let draw = |cx: &mut TestAppContext, window: WindowHandle<Screen>, forget: bool| {
        cx.update_window(window.into(), |_, window, cx| {
            if forget {
                window.forget_retained_state();
            }
            window.draw(cx).clear(cx);
            window.describe_rendered_frame()
        })
        .unwrap()
    };
    let [incremental, from_scratch] = windows;
    let expected = draw(cx, from_scratch, true);
    let actual = draw(cx, incremental, false);
    assert_eq!(actual, expected);
    let after = builds.get();
    let built = [0, 1, 2, 3, 4].map(|view| after[view] - before[view]);
    ([built[0], built[1], built[2], built[3]], built[4] > 0)
}

/// Moves the focus as [`step`] does, then draws a frame with nothing
/// changed, which must build nothing.
fn focus(
    cx: &mut TestAppContext,
    windows: [WindowHandle<Screen>; 2],
    builds: &Builds,
    tile: Option<usize>,
) -> ([usize; 4], bool) {
    let built = step(cx, windows, builds, Some(tile));
    assert_eq!(step(cx, windows, builds, None), ([0; 4], false));
    built
}

fn setup(cx: &mut TestAppContext) -> ([WindowHandle<Screen>; 2], Builds) {
    let builds = Builds::default();
    let windows = [open(cx, &builds), open(cx, &Builds::default())];
    for window in windows {
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
    }
    (windows, builds)
}

/// Focus moving from the first tile to the second builds those two again,
/// the panel around them, which draws them, and the status line, which read
/// the focus as a whole, but not the third tile, whose answer stayed no.
#[test]
fn focus_moving_from_one_view_to_another_keeps_a_third_retained() {
    let mut cx = TestAppContext::single();
    let (windows, builds) = setup(&mut cx);
    focus(&mut cx, windows, &builds, Some(0));
    //                                                  tiles   panel status
    assert_eq!(
        focus(&mut cx, windows, &builds, Some(1)),
        ([1, 1, 0, 1], true)
    );
}

/// The panel asks whether it contains the focus, and is built again when
/// its answer, or its tiles', change: not when the focus moves between two
/// views outside it.
#[test]
fn a_view_asking_whether_it_contains_the_focus_is_built_when_that_changes() {
    let mut cx = TestAppContext::single();
    let (windows, builds) = setup(&mut cx);
    focus(&mut cx, windows, &builds, Some(1));
    assert_eq!(
        focus(&mut cx, windows, &builds, Some(2)),
        ([0, 1, 1, 1], true)
    );
    assert_eq!(focus(&mut cx, windows, &builds, None), ([0, 0, 1, 0], true));
    assert_eq!(
        focus(&mut cx, windows, &builds, Some(0)),
        ([1, 0, 0, 1], true)
    );
}

/// Focusing what already has the focus builds nothing.
#[test]
fn focusing_the_focused_view_again_builds_nothing() {
    let mut cx = TestAppContext::single();
    let (windows, builds) = setup(&mut cx);
    focus(&mut cx, windows, &builds, Some(2));
    assert_eq!(focus(&mut cx, windows, &builds, Some(2)), ([0; 4], false));
}

/// A focusable box that, once told to, holds a focusable child marked while
/// the focus is within it, as a row reveals its actions while focused.
struct Host {
    focus: FocusHandle,
    shown: Rc<Cell<bool>>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let child = self.shown.get().then(|| {
            div()
                .id("child")
                .tab_index(0)
                .w(px(20.))
                .h(px(20.))
                .bg(hsla(0., 0., 0.5, 1.))
                .in_focus(|style| style.bg(hsla(0.6, 0.8, 0.5, 1.)))
        });
        div()
            .track_focus(&self.focus)
            .w(px(80.))
            .h(px(40.))
            .children(child)
    }
}

/// An element drawn for the first time inside the focused element is
/// within it in that first frame, as a frame drawn from scratch after it
/// has it: the focus did not move, so no later frame comes to put it right.
#[test]
fn an_element_drawn_first_inside_the_focused_one_is_within_it_at_once() {
    let mut cx = TestAppContext::single();
    let shown = Rc::new(Cell::new(false));
    let told = shown.clone();
    let window = cx.add_window(move |_, cx| Host {
        focus: cx.focus_handle(),
        shown: told,
    });
    window
        .update(&mut cx, |host, window, cx| window.focus(&host.focus, cx))
        .unwrap();
    // The window is drawn as each update ends: the child's first frame.
    window
        .update(&mut cx, |host, _, cx| {
            host.shown.set(true);
            cx.notify();
        })
        .unwrap();
    let first = cx
        .update_window(window.into(), |_, window, _| {
            window.describe_rendered_frame()
        })
        .unwrap();
    let from_scratch = cx
        .update_window(window.into(), |_, window, cx| {
            window.forget_retained_state();
            window.draw(cx).clear(cx);
            window.describe_rendered_frame()
        })
        .unwrap();
    assert!(shown.get());
    assert_eq!(first, from_scratch);
}
