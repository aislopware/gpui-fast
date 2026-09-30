//! Trackpad gestures besides the pinch: rotating two fingers, the two-finger
//! double tap AppKit calls smart magnify, and the three-finger swipe.
//!
//! Upstream GPUI delivers a pinch ([`crate::PinchEvent`]) and turns a swipe
//! into a back or forward mouse button. An application that forwards input
//! elsewhere, a remote desktop say, needs the gestures themselves, so the
//! platforms deliver them as [`PlatformGesture`]s, one
//! [`crate::PlatformInput::Gesture`], dispatched to mouse listeners by
//! position like a pinch. [`InteractiveGestures`] adds a listener for each to
//! every interactive element, and a custom element listens with
//! `window.on_mouse_event::<PlatformGesture>`.
//!
//! A swipe that nothing stops the propagation of goes on to be the back or
//! forward button it is upstream, so a view that takes a swipe for itself
//! calls `cx.stop_propagation()`.

use crate::{
    App, DispatchPhase, Hitbox, InputEvent, InteractiveElement, Modifiers, MouseButton,
    MouseDownEvent, MouseEvent, NavigationDirection, PlatformInput, Pixels, Point, TouchPhase,
    Window, seal::Sealed,
};
use std::ops::Deref;

/// A gesture from the platform, other than a pinch.
#[derive(Clone, Debug)]
pub enum PlatformGesture {
    /// Two fingers rotating.
    Rotate(RotateEvent),
    /// Two fingers tapping twice.
    SmartMagnify(SmartMagnifyEvent),
    /// Three fingers swiping.
    Swipe(SwipeEvent),
}

impl PlatformGesture {
    /// Where the pointer was.
    pub fn position(&self) -> Point<Pixels> {
        match self {
            PlatformGesture::Rotate(event) => event.position,
            PlatformGesture::SmartMagnify(event) => event.position,
            PlatformGesture::Swipe(event) => event.position,
        }
    }

    /// The modifiers held down.
    pub fn modifiers(&self) -> Modifiers {
        match self {
            PlatformGesture::Rotate(event) => event.modifiers,
            PlatformGesture::SmartMagnify(event) => event.modifiers,
            PlatformGesture::Swipe(event) => event.modifiers,
        }
    }
}

impl Sealed for PlatformGesture {}
impl InputEvent for PlatformGesture {
    fn to_platform_input(self) -> PlatformInput {
        PlatformInput::Gesture(self)
    }
}
impl MouseEvent for PlatformGesture {}

/// Two fingers rotating on a trackpad (`rotateWithEvent:` on macOS), or on
/// the screen or a trackpad (`UIRotationGestureRecognizer` on iOS).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RotateEvent {
    /// Where the pointer, or the fingers' centre on a touch screen, was.
    pub position: Point<Pixels>,
    /// The rotation since the previous event of the gesture, in degrees,
    /// counterclockwise as seen on the screen positive, as AppKit reports it.
    pub rotation: f32,
    /// The modifiers held down.
    pub modifiers: Modifiers,
    /// Where the gesture is: one `Started`, `Moved`s, then `Ended`, or
    /// `Cancelled` when the system took it back.
    pub phase: TouchPhase,
}

/// Two fingers tapping twice on a trackpad, AppKit's smart magnify
/// (`smartMagnifyWithEvent:`), which asks to zoom to what is under the
/// pointer or back out. It carries no amount.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SmartMagnifyEvent {
    /// Where the pointer was.
    pub position: Point<Pixels>,
    /// The modifiers held down.
    pub modifiers: Modifiers,
}

/// A swipe across a trackpad, with three fingers where the user set
/// “Swipe between pages” to use them (`swipeWithEvent:`), or the swipe some
/// mice send for their back and forward buttons.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SwipeEvent {
    /// Where the pointer was.
    pub position: Point<Pixels>,
    /// AppKit's `deltaX` and `deltaY`: one of them is 1 or -1 and the other
    /// 0. `x` is 1 for what AppKit calls a swipe left, which goes back a
    /// page, and -1 for a swipe right, which goes forward; `y` is 1 for a
    /// swipe up and -1 for a swipe down.
    pub delta: Point<f32>,
    /// The modifiers held down.
    pub modifiers: Modifiers,
    /// `Ended` for a swipe that arrives whole, as AppKit's do; the phase
    /// the platform reports otherwise.
    pub phase: TouchPhase,
}

macro_rules! gesture_event {
    ($event:ident, $variant:ident) => {
        impl Sealed for $event {}
        impl InputEvent for $event {
            fn to_platform_input(self) -> PlatformInput {
                PlatformInput::Gesture(PlatformGesture::$variant(self))
            }
        }
        impl Deref for $event {
            type Target = Modifiers;

            fn deref(&self) -> &Modifiers {
                &self.modifiers
            }
        }
    };
}

gesture_event!(RotateEvent, Rotate);
gesture_event!(SmartMagnifyEvent, SmartMagnify);
gesture_event!(SwipeEvent, Swipe);

pub(crate) type GestureListener =
    Box<dyn Fn(&PlatformGesture, DispatchPhase, &Hitbox, &mut Window, &mut App) + 'static>;

/// Listeners for the gestures of [`PlatformGesture`], on every interactive
/// element. Each is called in the bubble phase while the element is hovered,
/// as a pinch listener is.
pub trait InteractiveGestures: InteractiveElement {
    /// Binds `listener` to two fingers rotating over the element.
    fn on_rotate(self, listener: impl Fn(&RotateEvent, &mut Window, &mut App) + 'static) -> Self {
        on_gesture(self, move |gesture, window, cx| {
            if let PlatformGesture::Rotate(event) = gesture {
                listener(event, window, cx);
            }
        })
    }

    /// Binds `listener` to two fingers tapping twice over the element.
    fn on_smart_magnify(
        self,
        listener: impl Fn(&SmartMagnifyEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        on_gesture(self, move |gesture, window, cx| {
            if let PlatformGesture::SmartMagnify(event) = gesture {
                listener(event, window, cx);
            }
        })
    }

    /// Binds `listener` to a swipe over the element. A listener that takes
    /// the swipe calls `cx.stop_propagation()`, or it goes on to be a back
    /// or forward mouse button.
    fn on_swipe(self, listener: impl Fn(&SwipeEvent, &mut Window, &mut App) + 'static) -> Self {
        on_gesture(self, move |gesture, window, cx| {
            if let PlatformGesture::Swipe(event) = gesture {
                listener(event, window, cx);
            }
        })
    }
}

impl<E: InteractiveElement> InteractiveGestures for E {}

fn on_gesture<E: InteractiveElement>(
    mut element: E,
    listener: impl Fn(&PlatformGesture, &mut Window, &mut App) + 'static,
) -> E {
    element
        .interactivity()
        .fast_gesture_listeners
        .push(Box::new(move |gesture, phase, hitbox, window, cx| {
            if phase == DispatchPhase::Bubble && hitbox.is_hovered(window) {
                listener(gesture, window, cx);
            }
        }));
    element
}

/// Whether an element listens for a gesture, for it to take a hitbox.
pub(crate) fn any_listener(
    listeners: &crate::fast::interactivity::LazyVec<GestureListener>,
) -> bool {
    !listeners.is_empty()
}

/// Registers an element's gesture listeners for this frame, on its hitbox.
pub(crate) fn paint_listeners(
    listeners: &mut crate::fast::interactivity::LazyVec<GestureListener>,
    hitbox: &Hitbox,
    window: &mut Window,
) {
    for listener in listeners.drain(..) {
        let hitbox = hitbox.clone();
        window.on_mouse_event(move |gesture: &PlatformGesture, phase, window, cx| {
            listener(gesture, phase, &hitbox, window, cx);
        });
    }
}

/// Dispatches a horizontal swipe that went through unstopped as the back or
/// forward mouse button upstream makes of it, and answers for both.
pub(crate) fn navigate_unstopped_swipe(
    window: &mut Window,
    event: &PlatformInput,
    cx: &mut App,
) -> Option<crate::DispatchEventResult> {
    let PlatformInput::Gesture(PlatformGesture::Swipe(swipe)) = event else {
        return None;
    };
    if !cx.propagate_event || swipe.phase != TouchPhase::Ended {
        return None;
    }
    let direction = if swipe.delta.x > 0. {
        NavigationDirection::Back
    } else if swipe.delta.x < 0. {
        NavigationDirection::Forward
    } else {
        return None;
    };
    Some(window.dispatch_event(
        PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Navigate(direction),
            position: swipe.position,
            modifiers: swipe.modifiers,
            click_count: 1,
            first_mouse: false,
        }),
        cx,
    ))
}
