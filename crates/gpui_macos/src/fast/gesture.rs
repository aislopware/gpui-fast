//! The trackpad's rotate, smart magnify and swipe events, as
//! [`gpui::PlatformGesture`]s.
//!
//! AppKit reports a rotation as the change since the previous rotate event,
//! in degrees counterclockwise, with the gesture's phase; a smart magnify
//! (two fingers tapping twice) as a lone event with a place; and a swipe
//! (three fingers, where the user set “Swipe between pages” to use them, or
//! a mouse's back and forward buttons) as one event whose `deltaX` or
//! `deltaY` is 1 or -1.

use cocoa::{
    appkit::{NSEvent, NSEventPhase},
    base::id,
};
use gpui::{
    Modifiers, Pixels, PlatformGesture, PlatformInput, RotateEvent, SmartMagnifyEvent, SwipeEvent,
    TouchPhase, point, px,
};
use objc::{msg_send, sel, sel_impl};

/// `NSEventTypeRotate`.
const ROTATE: u64 = 18;
/// `NSEventTypeSwipe`.
const SWIPE: u64 = 31;
/// `NSEventTypeSmartMagnify`, which the `cocoa` crate's `NSEventType` lacks.
const SMART_MAGNIFY: u64 = 32;

/// What GPUI reads off a gesture's `NSEvent`.
#[derive(Clone, Copy, Debug)]
struct Native {
    event_type: u64,
    phase: NSEventPhase,
    /// `locationInWindow`, from the bottom left.
    location: (f64, f64),
    rotation: f32,
    delta: (f64, f64),
    modifiers: Modifiers,
}

/// The gesture `native_event` is, when it is one GPUI delivers as a
/// [`PlatformGesture`]; `None` leaves it to the rest of the conversion.
pub(crate) unsafe fn from_native(
    native_event: id,
    window_height: Option<Pixels>,
) -> Option<PlatformInput> {
    let window_height = window_height?;
    // SAFETY: every NSEvent answers `type`, an NSUInteger; read raw, since
    // smart magnify is not a value of the `cocoa` crate's enum.
    let event_type: u64 = unsafe { msg_send![native_event, type] };
    if !matches!(event_type, ROTATE | SWIPE | SMART_MAGNIFY) {
        return None;
    }
    // SAFETY: a gesture NSEvent answers `phase`, `locationInWindow`,
    // `rotation` and the deltas; `rotation` and the deltas are 0 where they
    // mean nothing.
    let native = unsafe {
        let location = native_event.locationInWindow();
        Native {
            event_type,
            phase: native_event.phase(),
            location: (location.x, location.y),
            rotation: if event_type == ROTATE {
                native_event.rotation()
            } else {
                0.
            },
            delta: if event_type == SWIPE {
                (native_event.deltaX(), native_event.deltaY())
            } else {
                (0., 0.)
            },
            modifiers: crate::events::read_modifiers(native_event),
        }
    };
    gesture(native, window_height).map(PlatformInput::Gesture)
}

fn gesture(native: Native, window_height: Pixels) -> Option<PlatformGesture> {
    let position = point(
        px(native.location.0 as f32),
        window_height - px(native.location.1 as f32),
    );
    let modifiers = native.modifiers;
    match native.event_type {
        ROTATE => Some(PlatformGesture::Rotate(RotateEvent {
            position,
            rotation: native.rotation,
            modifiers,
            // A rotation that may begin, or has no phase, has not rotated.
            phase: phase(native.phase)?,
        })),
        SMART_MAGNIFY => Some(PlatformGesture::SmartMagnify(SmartMagnifyEvent {
            position,
            modifiers,
        })),
        SWIPE => Some(PlatformGesture::Swipe(SwipeEvent {
            position,
            delta: point(native.delta.0 as f32, native.delta.1 as f32),
            modifiers,
            // A swipe arrives whole.
            phase: phase(native.phase).unwrap_or(TouchPhase::Ended),
        })),
        _ => None,
    }
}

fn phase(phase: NSEventPhase) -> Option<TouchPhase> {
    if phase.contains(NSEventPhase::NSEventPhaseBegan) {
        Some(TouchPhase::Started)
    } else if phase.intersects(NSEventPhase::NSEventPhaseChanged | NSEventPhase::NSEventPhaseStationary)
    {
        Some(TouchPhase::Moved)
    } else if phase.contains(NSEventPhase::NSEventPhaseEnded) {
        Some(TouchPhase::Ended)
    } else if phase.contains(NSEventPhase::NSEventPhaseCancelled) {
        Some(TouchPhase::Cancelled)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{Native, ROTATE, SMART_MAGNIFY, SWIPE, gesture};
    use cocoa::appkit::NSEventPhase;
    use gpui::{
        Modifiers, PlatformGesture, RotateEvent, SmartMagnifyEvent, SwipeEvent, TouchPhase, point,
        px,
    };

    fn native(event_type: u64, phase: NSEventPhase) -> Native {
        Native {
            event_type,
            phase,
            location: (40., 100.),
            rotation: 0.,
            delta: (0., 0.),
            modifiers: Modifiers::default(),
        }
    }

    #[test]
    fn a_rotation_keeps_appkits_degrees_and_phases_in_window_coordinates() {
        let phases = [
            (NSEventPhase::NSEventPhaseBegan, TouchPhase::Started),
            (NSEventPhase::NSEventPhaseChanged, TouchPhase::Moved),
            (NSEventPhase::NSEventPhaseEnded, TouchPhase::Ended),
            (NSEventPhase::NSEventPhaseCancelled, TouchPhase::Cancelled),
        ];
        for (native_phase, phase) in phases {
            let event = gesture(
                Native {
                    rotation: -2.5,
                    modifiers: Modifiers::command(),
                    ..native(ROTATE, native_phase)
                },
                px(600.),
            );
            let Some(PlatformGesture::Rotate(event)) = event else {
                panic!("not a rotation: {event:?}");
            };
            assert_eq!(
                event,
                RotateEvent {
                    position: point(px(40.), px(500.)),
                    rotation: -2.5,
                    modifiers: Modifiers::command(),
                    phase,
                }
            );
        }
    }

    #[test]
    fn a_rotation_that_may_begin_is_not_delivered() {
        for phase in [
            NSEventPhase::NSEventPhaseMayBegin,
            NSEventPhase::NSEventPhaseNone,
        ] {
            assert!(gesture(native(ROTATE, phase), px(600.)).is_none());
        }
    }

    #[test]
    fn a_smart_magnify_carries_its_place() {
        let event = gesture(
            native(SMART_MAGNIFY, NSEventPhase::NSEventPhaseNone),
            px(600.),
        );
        let Some(PlatformGesture::SmartMagnify(event)) = event else {
            panic!("not a smart magnify: {event:?}");
        };
        assert_eq!(
            event,
            SmartMagnifyEvent {
                position: point(px(40.), px(500.)),
                modifiers: Modifiers::default(),
            }
        );
    }

    #[test]
    fn a_swipe_keeps_appkits_deltas_and_arrives_ended() {
        for (delta, native_phase, phase) in [
            ((1., 0.), NSEventPhase::NSEventPhaseNone, TouchPhase::Ended),
            ((-1., 0.), NSEventPhase::NSEventPhaseEnded, TouchPhase::Ended),
            ((0., 1.), NSEventPhase::NSEventPhaseBegan, TouchPhase::Started),
            ((0., -1.), NSEventPhase::NSEventPhaseChanged, TouchPhase::Moved),
        ] {
            let event = gesture(
                Native {
                    delta,
                    ..native(SWIPE, native_phase)
                },
                px(600.),
            );
            let Some(PlatformGesture::Swipe(event)) = event else {
                panic!("not a swipe: {event:?}");
            };
            assert_eq!(
                event,
                SwipeEvent {
                    position: point(px(40.), px(500.)),
                    delta: point(delta.0 as f32, delta.1 as f32),
                    modifiers: Modifiers::default(),
                    phase,
                }
            );
        }
    }

    #[test]
    fn other_events_are_left_to_the_rest_of_the_conversion() {
        // NSEventTypeMagnify, the pinch.
        assert!(gesture(native(30, NSEventPhase::NSEventPhaseBegan), px(600.)).is_none());
    }
}
