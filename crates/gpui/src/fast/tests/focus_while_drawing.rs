//! A view that moves the focus, or notifies another view, as it renders, as
//! a workspace gives the keyboard to the tile it was asked to. The views
//! drawn after it in that frame, drawn again from last frame or not, show
//! the change; the views drawn before it are drawn again in the next frame
//! where the focus moved; and a render that leaves the focus where it was
//! asks for no frame. See [`crate::fast::focus::focus_changed`] and
//! [`crate::fast::splice`].
//!
//! Each frame of a window changed while it draws is compared with one, drawn
//! from scratch, of a window changed before it.

use crate::{
    AnyView, AppContext as _, Context, Entity, EntityId, FocusHandle, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, StyleRefinement, Styled as _, TestAppContext, Window,
    WindowHandle, div, hsla, px,
};
use std::{cell::Cell, rc::Rc};

/// A focusable box, blue while focused, wider while its render saw it
/// focused and by however much it is told to be, counting its builds.
struct Tile {
    focus: FocusHandle,
    wider: Rc<Cell<f32>>,
    builds: Rc<Cell<usize>>,
}

impl Render for Tile {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.builds.set(self.builds.get() + 1);
        let focused = self.focus.is_focused(window);
        div()
            .track_focus(&self.focus)
            .w(px(if focused { 60. } else { 40. } + self.wider.get()))
            .h(px(20.))
            .bg(hsla(0., 0., 0.5, 1.))
            .focus(|style| style.bg(hsla(0.6, 0.8, 0.5, 1.)))
    }
}

/// A view around a tile, reading nothing itself.
struct Panel {
    tile: Entity<Tile>,
}

impl Render for Panel {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().p_1().child(self.tile.clone())
    }
}

/// What the mover does the next time it renders.
#[derive(Clone)]
enum Change {
    Focus(FocusHandle),
    Blur,
    /// Focuses one handle, then the other.
    Bounce(FocusHandle, FocusHandle),
    /// Widens a tile, whose view it notifies.
    Widen(Rc<Cell<f32>>, EntityId),
}

impl Change {
    fn make(self, window: &mut Window, cx: &mut crate::App) {
        match self {
            Change::Focus(handle) => window.focus(&handle, cx),
            Change::Blur => window.blur(cx),
            Change::Bounce(away, back) => {
                window.focus(&away, cx);
                window.focus(&back, cx);
            }
            Change::Widen(wider, view) => {
                wider.set(wider.get() + 10.);
                cx.notify(view);
            }
        }
    }
}

/// A view that changes the window as it renders, then draws its tile.
struct Mover {
    next: Option<Change>,
    tile: Entity<Tile>,
}

impl Render for Mover {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(change) = self.next.take() {
            change.make(window, cx);
        }
        div().pt_1().child(self.tile.clone())
    }
}

/// A tile drawn before the mover, the mover holding a second, then one drawn
/// as a cached view and one nested in a panel.
struct Screen {
    before: Entity<Tile>,
    mover: Entity<Mover>,
    cached: Entity<Tile>,
    panel: Entity<Panel>,
}

impl Render for Screen {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let mut size = StyleRefinement::default();
        size.size.width = Some(px(100.).into());
        size.size.height = Some(px(20.).into());
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(self.before.clone())
            .child(self.mover.clone())
            .child(AnyView::from(self.cached.clone()).cached(size))
            .child(self.panel.clone())
    }
}

/// The tiles: before the mover, held by it, cached, and nested in the panel.
const TILES: [usize; 4] = [0, 1, 2, 3];

#[derive(Default)]
struct Builds([Rc<Cell<usize>>; 4]);

impl Builds {
    fn get(&self) -> [usize; 4] {
        self.0.each_ref().map(|builds| builds.get())
    }
}

fn open(cx: &mut TestAppContext, builds: &Builds) -> WindowHandle<Screen> {
    let builds = builds.0.clone();
    cx.add_window(move |_, cx| {
        let [before, held, cached, nested] = builds.map(|builds| {
            cx.new(|cx| Tile {
                focus: cx.focus_handle(),
                wider: Rc::default(),
                builds,
            })
        });
        Screen {
            before,
            mover: cx.new(|_| Mover {
                next: None,
                tile: held,
            }),
            cached,
            panel: cx.new(|_| Panel { tile: nested }),
        }
    })
}

/// A window's tiles.
fn tiles(cx: &mut TestAppContext, window: WindowHandle<Screen>) -> [Entity<Tile>; 4] {
    window
        .read_with(cx, |screen, cx| {
            [
                screen.before.clone(),
                screen.mover.read(cx).tile.clone(),
                screen.cached.clone(),
                screen.panel.read(cx).tile.clone(),
            ]
        })
        .unwrap()
}

/// A change to a window's tiles.
#[derive(Clone, Copy)]
enum To {
    Focus(usize),
    Blur,
    Bounce(usize, usize),
    Widen(usize),
}

impl To {
    fn change(self, cx: &mut TestAppContext, window: WindowHandle<Screen>) -> Change {
        let tiles = tiles(cx, window);
        cx.read(|cx| match self {
            To::Focus(tile) => Change::Focus(tiles[tile].read(cx).focus.clone()),
            To::Blur => Change::Blur,
            To::Bounce(away, back) => Change::Bounce(
                tiles[away].read(cx).focus.clone(),
                tiles[back].read(cx).focus.clone(),
            ),
            To::Widen(tile) => {
                Change::Widen(tiles[tile].read(cx).wider.clone(), tiles[tile].entity_id())
            }
        })
    }
}

/// A window changed as its mover renders, and a reference window changed
/// before it draws.
struct Windows {
    moving: WindowHandle<Screen>,
    reference: WindowHandle<Screen>,
    builds: Builds,
    /// Whether the screen is notified along with the mover, so that it is
    /// built, and the tile before the mover laid out before it renders,
    /// rather than drawn from last frame around the mover.
    screen_built: bool,
}

/// A frame of the moving window.
#[derive(Debug, PartialEq)]
struct Frame {
    /// How often each tile was built for it.
    built: [usize; 4],
    /// Whether it shows what the reference window does.
    current: bool,
    /// Whether the window asked for another frame once it was drawn.
    asks: bool,
}

/// A frame that builds nothing, is current and asks for no other.
const IDLE: Frame = Frame {
    built: [0; 4],
    current: true,
    asks: false,
};

impl Windows {
    /// Opens both windows, focusing `focus` in both, and draws them.
    fn open(cx: &mut TestAppContext, focus: Option<usize>, screen_built: bool) -> Self {
        let builds = Builds::default();
        let windows = Windows {
            moving: open(cx, &builds),
            reference: open(cx, &Builds::default()),
            builds,
            screen_built,
        };
        for window in [windows.moving, windows.reference] {
            let tiles = tiles(cx, window);
            cx.update_window(window.into(), |_, window, cx| {
                if let Some(tile) = focus {
                    window.focus(&tiles[tile].read(cx).focus.clone(), cx);
                }
                window.draw(cx).clear(cx);
            })
            .unwrap();
        }
        windows
    }

    /// Has the moving window's mover make the change `to` as it renders in
    /// its next frame, and makes it in the reference window now.
    fn change_while_drawing(&self, cx: &mut TestAppContext, to: To) {
        let change = to.change(cx, self.moving);
        self.moving
            .update(cx, |screen, _, cx| {
                screen
                    .mover
                    .update(cx, |mover, _| mover.next = Some(change));
            })
            .unwrap();
        let change = to.change(cx, self.reference);
        cx.update_window(self.reference.into(), |_, window, cx| {
            change.make(window, cx);
        })
        .unwrap();
    }

    /// Draws a frame of the moving window, its mover notified when it has a
    /// change to make, and compares it with the reference window's drawn
    /// from scratch. A frame the moving window asks for once it is drawn is
    /// drawn as the update ends, as the test platform draws every dirty
    /// window; it is left for the next call to compare.
    fn frame(&self, cx: &mut TestAppContext) -> Frame {
        let expected = cx
            .update_window(self.reference.into(), |_, window, cx| {
                window.forget_retained_state();
                window.draw(cx).clear(cx);
                window.describe_rendered_frame()
            })
            .unwrap();
        let before = self.builds.get();
        let (screen, mover) = self
            .moving
            .update(cx, |screen, _, cx| {
                let mover = &screen.mover;
                let changes = mover.read(cx).next.is_some();
                (
                    (changes && self.screen_built).then(|| cx.entity_id()),
                    changes.then(|| mover.entity_id()),
                )
            })
            .unwrap();
        let (actual, asks, built) = cx
            .update_window(self.moving.into(), |_, window, cx| {
                for view in screen.into_iter().chain(mover) {
                    cx.notify(view);
                }
                window.draw(cx).clear(cx);
                let after = self.builds.get();
                (
                    window.describe_rendered_frame(),
                    window.invalidator.is_dirty(),
                    TILES.map(|tile| after[tile] - before[tile]),
                )
            })
            .unwrap();
        Frame {
            built,
            current: actual == expected,
            asks,
        }
    }
}

/// `tiles` built once each.
fn built(tiles: &[usize]) -> [usize; 4] {
    TILES.map(|tile| usize::from(tiles.contains(&tile)))
}

/// The focus moving, as the mover renders, between views drawn after it is
/// shown in that frame: the view it left and the view it reached are built
/// again, whether the mover's own, drawn as a cached view, or nested in a
/// view drawn again around them. The screen around the mover is built, or
/// drawn again from last frame around it, which a move inside it makes it
/// build after all. The window asks for a frame, as any focus move does,
/// and that frame builds nothing, as nothing drawn before the move read the
/// focus.
#[test]
fn views_drawn_after_the_focus_moves_while_drawing_show_it_in_that_frame() {
    for screen_built in [true, false] {
        let mut cx = TestAppContext::single();
        let windows = Windows::open(&mut cx, Some(1), screen_built);
        for (from, to) in [(1, 2), (2, 3), (3, 1)] {
            windows.change_while_drawing(&mut cx, To::Focus(to));
            let moved = Frame {
                built: built(&[from, to]),
                current: true,
                asks: true,
            };
            let frame = windows.frame(&mut cx);
            assert_eq!(
                frame, moved,
                "from {from} to {to}, screen built: {screen_built}"
            );
            assert_eq!(windows.frame(&mut cx), IDLE);
        }
    }
}

/// A view laid out before the focus moved while drawing shows the focus as
/// it was, so the window asks for another frame, which builds it again, and
/// only it. Where the screen around the mover was to be drawn from last
/// frame around it, the move has it built after all, and the view before
/// the mover with it, so the frame shows the move whole.
#[test]
fn a_view_drawn_before_the_focus_moves_while_drawing_is_drawn_again() {
    let mut cx = TestAppContext::single();
    let windows = Windows::open(&mut cx, Some(0), true);
    windows.change_while_drawing(&mut cx, To::Focus(2));
    let builds = windows.builds.get();
    let moved = Frame {
        built: built(&[2]),
        current: false,
        asks: true,
    };
    assert_eq!(windows.frame(&mut cx), moved);
    let after = windows.builds.get();
    assert_eq!(
        TILES.map(|tile| after[tile] - builds[tile]),
        built(&[0, 2]),
        "the frame asked for builds the view drawn before the move"
    );
    assert_eq!(windows.frame(&mut cx), IDLE);

    let mut cx = TestAppContext::single();
    let windows = Windows::open(&mut cx, Some(0), false);
    windows.change_while_drawing(&mut cx, To::Focus(2));
    let moved = Frame {
        built: built(&[0, 2]),
        current: true,
        asks: true,
    };
    assert_eq!(windows.frame(&mut cx), moved);
    assert_eq!(windows.frame(&mut cx), IDLE);
}

/// A render that blurs a window with nothing focused, or focuses what has
/// the focus, moves nothing, so it builds nothing and asks for no frame: a
/// view doing so each time it is built cannot keep its window drawing. Nor
/// can one whose render moves the focus away and back.
#[test]
fn a_render_leaving_the_focus_where_it_was_asks_for_no_frame() {
    for screen_built in [true, false] {
        let mut cx = TestAppContext::single();
        let windows = Windows::open(&mut cx, None, screen_built);
        windows.change_while_drawing(&mut cx, To::Blur);
        assert_eq!(windows.frame(&mut cx), IDLE);

        windows.change_while_drawing(&mut cx, To::Focus(3));
        windows.frame(&mut cx);
        assert_eq!(windows.frame(&mut cx), IDLE);
        windows.change_while_drawing(&mut cx, To::Focus(3));
        assert_eq!(windows.frame(&mut cx), IDLE);

        windows.change_while_drawing(&mut cx, To::Bounce(1, 3));
        let frame = windows.frame(&mut cx);
        assert!(
            frame.current && !frame.asks,
            "{frame:?}, screen built: {screen_built}"
        );
    }
}

/// A view notified as the mover renders is built in that frame when it is
/// drawn after the mover, around it or not. A notification while drawing
/// asks for no frame, and stands for the next frame too, which builds the
/// view once more, as upstream has it.
#[test]
fn a_view_notified_while_drawing_is_built_in_that_frame_when_drawn_after() {
    for screen_built in [true, false] {
        let mut cx = TestAppContext::single();
        let windows = Windows::open(&mut cx, None, screen_built);
        for tile in [1, 2, 3] {
            windows.change_while_drawing(&mut cx, To::Widen(tile));
            let widened = Frame {
                built: built(&[tile]),
                current: true,
                asks: false,
            };
            let frame = windows.frame(&mut cx);
            assert_eq!(frame, widened, "tile {tile}, screen built: {screen_built}");
            let next = Frame {
                built: built(&[tile]),
                current: true,
                asks: false,
            };
            assert_eq!(windows.frame(&mut cx), next);
            assert_eq!(windows.frame(&mut cx), IDLE);
        }
    }
}
