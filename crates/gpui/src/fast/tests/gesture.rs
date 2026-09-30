use crate::{
    self as gpui, Context, InteractiveElement, InteractiveGestures, IntoElement, Modifiers,
    MouseButton, NavigationDirection, ParentElement, Render, RotateEvent, SmartMagnifyEvent,
    Styled, SwipeEvent, TestAppContext, TouchPhase, VisualTestContext, Window, div, point, px,
};

/// What a view saw of the gestures, and of the back and forward buttons.
#[derive(Default)]
struct Pad {
    rotations: Vec<f32>,
    magnified: usize,
    swipes: Vec<(f32, f32)>,
    navigated: Vec<NavigationDirection>,
    /// Whether the view takes swipes for itself.
    takes_swipes: bool,
}

impl Render for Pad {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(
                div()
                    .id("pad")
                    .w(px(100.))
                    .h(px(100.))
                    .on_rotate(cx.listener(|pad, event: &RotateEvent, _, _| {
                        pad.rotations.push(event.rotation);
                    }))
                    .on_smart_magnify(cx.listener(|pad, _: &SmartMagnifyEvent, _, _| {
                        pad.magnified += 1;
                    }))
                    .on_swipe(cx.listener(|pad, event: &SwipeEvent, _, cx| {
                        pad.swipes.push((event.delta.x, event.delta.y));
                        if pad.takes_swipes {
                            cx.stop_propagation();
                        }
                    })),
            )
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(|pad, _, _, _| pad.navigated.push(NavigationDirection::Back)),
            )
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Forward),
                cx.listener(|pad, _, _, _| pad.navigated.push(NavigationDirection::Forward)),
            )
    }
}

fn pad(cx: &mut TestAppContext) -> (gpui::Entity<Pad>, &mut VisualTestContext) {
    let (pad, cx) = cx.add_window_view(|_, _| Pad::default());
    cx.run_until_parked();
    (pad, cx)
}

fn rotate(at: f32, rotation: f32) -> RotateEvent {
    RotateEvent {
        position: point(px(at), px(50.)),
        rotation,
        modifiers: Modifiers::default(),
        phase: TouchPhase::Moved,
    }
}

fn swipe(x: f32) -> SwipeEvent {
    SwipeEvent {
        position: point(px(50.), px(50.)),
        delta: point(x, 0.),
        modifiers: Modifiers::default(),
        phase: TouchPhase::Ended,
    }
}

#[gpui::test]
fn gestures_reach_the_element_under_the_pointer_only(cx: &mut TestAppContext) {
    let (pad, cx) = pad(cx);
    cx.simulate_event(rotate(50., 12.5));
    cx.simulate_event(rotate(300., 7.));
    cx.simulate_event(SmartMagnifyEvent {
        position: point(px(20.), px(20.)),
        modifiers: Modifiers::default(),
    });
    cx.simulate_event(SmartMagnifyEvent {
        position: point(px(200.), px(200.)),
        modifiers: Modifiers::default(),
    });
    pad.read_with(cx, |pad, _| {
        assert_eq!(pad.rotations, [12.5]);
        assert_eq!(pad.magnified, 1);
    });
}

#[gpui::test]
fn a_swipe_nothing_takes_goes_back_or_forward(cx: &mut TestAppContext) {
    let (pad, cx) = pad(cx);
    cx.simulate_event(swipe(1.));
    cx.simulate_event(swipe(-1.));
    // Vertical, and one still under way, are not a page's back or forward.
    cx.simulate_event(SwipeEvent {
        delta: point(0., 1.),
        ..swipe(0.)
    });
    cx.simulate_event(SwipeEvent {
        phase: TouchPhase::Moved,
        ..swipe(1.)
    });
    pad.read_with(cx, |pad, _| {
        assert_eq!(pad.swipes, [(1., 0.), (-1., 0.), (0., 1.), (1., 0.)]);
        assert_eq!(
            pad.navigated,
            [NavigationDirection::Back, NavigationDirection::Forward]
        );
    });
}

#[gpui::test]
fn a_swipe_taken_goes_nowhere_else(cx: &mut TestAppContext) {
    let (pad, cx) = pad(cx);
    pad.update(cx, |pad, _| pad.takes_swipes = true);
    cx.simulate_event(swipe(1.));
    pad.read_with(cx, |pad, _| {
        assert_eq!(pad.swipes, [(1., 0.)]);
        assert!(pad.navigated.is_empty());
    });
}
