//! The phase of the momentum AppKit sends after the fingers lift off the
//! trackpad, which a scroll's own phase does not tell apart from a wheel's.

use cocoa::{
    appkit::{NSEvent, NSEventPhase},
    base::id,
};
use gpui::TouchPhase;

/// Where `native_event`, a scroll wheel event, is in the momentum after a
/// swipe; `None` when a finger or a wheel drives it.
pub(crate) unsafe fn momentum_phase(native_event: id) -> Option<TouchPhase> {
    // SAFETY: the caller passes a scroll wheel NSEvent, which answers
    // momentumPhase.
    let phase = unsafe { native_event.momentumPhase() };
    match phase {
        NSEventPhase::NSEventPhaseBegan => Some(TouchPhase::Started),
        NSEventPhase::NSEventPhaseChanged => Some(TouchPhase::Moved),
        NSEventPhase::NSEventPhaseEnded => Some(TouchPhase::Ended),
        NSEventPhase::NSEventPhaseCancelled => Some(TouchPhase::Cancelled),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use gpui::{PlatformInput, ScrollDelta, TouchPhase, px};
    use objc2_app_kit::NSEvent;
    use objc2_core_graphics::{
        CGEvent, CGEventField, CGMomentumScrollPhase, CGScrollEventUnit, CGScrollPhase,
    };

    /// What GPUI makes of a scroll wheel event carrying these phases, as the
    /// window server would deliver it.
    fn delivered(
        scroll: CGScrollPhase,
        momentum: CGMomentumScrollPhase,
    ) -> (TouchPhase, Option<TouchPhase>, bool) {
        let event = CGEvent::new_scroll_wheel_event2(None, CGScrollEventUnit::Pixel, 1, -3, 0, 0)
            .expect("a scroll wheel CGEvent");
        CGEvent::set_integer_value_field(
            Some(&event),
            CGEventField::ScrollWheelEventScrollPhase,
            i64::from(scroll.0),
        );
        CGEvent::set_integer_value_field(
            Some(&event),
            CGEventField::ScrollWheelEventMomentumPhase,
            i64::from(momentum.0),
        );
        let native = NSEvent::eventWithCGEvent(&event).expect("an NSEvent");
        // SAFETY: `native` is a live scroll wheel NSEvent for the whole call.
        let input = unsafe {
            crate::events::platform_input_from_native(
                objc2::rc::Retained::as_ptr(&native) as cocoa::base::id,
                Some(px(600.)),
            )
        };
        let Some(PlatformInput::ScrollWheel(event)) = input else {
            panic!("not a scroll: {input:?}");
        };
        (
            event.touch_phase,
            event.momentum_phase,
            matches!(event.delta, ScrollDelta::Pixels(_)),
        )
    }

    #[test]
    fn momentum_is_told_from_a_swipe_and_from_a_wheel() {
        let none = CGScrollPhase(0);
        let swipe = [
            CGScrollPhase::Began,
            CGScrollPhase::Changed,
            CGScrollPhase::Ended,
        ]
        .map(|phase| delivered(phase, CGMomentumScrollPhase::None));
        let momentum = [
            CGMomentumScrollPhase::Begin,
            CGMomentumScrollPhase::Continue,
            CGMomentumScrollPhase::End,
        ]
        .map(|phase| delivered(none, phase));
        let wheel = delivered(none, CGMomentumScrollPhase::None);
        assert_eq!(
            swipe.map(|(phase, momentum, _)| (phase, momentum)),
            [
                (TouchPhase::Started, None),
                (TouchPhase::Moved, None),
                (TouchPhase::Ended, None),
            ]
        );
        assert_eq!(
            momentum.map(|(phase, momentum, _)| (phase, momentum)),
            [
                (TouchPhase::Moved, Some(TouchPhase::Started)),
                (TouchPhase::Moved, Some(TouchPhase::Moved)),
                (TouchPhase::Moved, Some(TouchPhase::Ended)),
            ]
        );
        assert_eq!((wheel.0, wheel.1), (TouchPhase::Moved, None));
    }
}
