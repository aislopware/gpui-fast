//! Conversion from UIKit events to GPUI input events.

use std::time::{Duration, Instant};

use gpui::{Pixels, Point, TouchId, px};
use objc2::msg_send;
use objc2::runtime::AnyObject;
use objc2_foundation::NSProcessInfo;

use super::cg_types::ObjcCGPoint;
use crate::described::UiTouchPhase;

/// Returns the touch position in window coordinates.
pub fn touch_location_in_view(touch: *mut AnyObject, view: *mut AnyObject) -> Point<Pixels> {
    unsafe {
        let location: ObjcCGPoint = msg_send![touch, locationInView: view];
        Point::new(px(location.x as f32), px(location.y as f32))
    }
}

/// When `touch` was last in its phase: `UITouch.timestamp`, the hardware's time of the event
/// UIKit read it from, in seconds since the system started ([`instant_of_uptime`]).
pub fn touch_timestamp(touch: *mut AnyObject) -> f64 {
    // SAFETY: `UITouch.timestamp` is a plain `NSTimeInterval` property, read on the main thread
    // that delivers the touch.
    unsafe { msg_send![touch, timestamp] }
}

/// The instant `uptime` seconds after the system started (the clock of `UITouch.timestamp`
/// and `UIEvent.timestamp`, `NSProcessInfo.systemUptime`'s), read against both clocks now; a
/// time that is not a time, or one ahead of now, is now.
pub fn instant_of_uptime(uptime: f64) -> Instant {
    let now = Instant::now();
    let age = NSProcessInfo::processInfo().systemUptime() - uptime;
    Duration::try_from_secs_f64(age)
        .ok()
        .and_then(|age| now.checked_sub(age))
        .unwrap_or(now)
}

/// Returns the current UIKit touch phase.
pub fn touch_phase(touch: *mut AnyObject) -> UiTouchPhase {
    unsafe {
        let phase: i64 = msg_send![touch, phase];
        UiTouchPhase::from_raw(phase)
    }
}

/// `UITouch.type` (`UITouchType`): direct, indirect, pencil or indirect pointer.
pub fn touch_type(touch: *mut AnyObject) -> i64 {
    // SAFETY: `UITouch.type` is a plain `NSInteger` property, read on the main thread that
    // delivers the touch.
    unsafe { msg_send![touch, type] }
}

/// `UIEvent.buttonMask` (`UIEventButtonMask`): the pointer's buttons held, 0 for a finger or
/// with no event.
pub fn button_mask(event: *mut AnyObject) -> usize {
    if event.is_null() {
        return 0;
    }
    // SAFETY: `UIEvent.buttonMask` (iOS 13.4+) is a plain `NSInteger` bit field, read on the
    // main thread that delivers the event.
    let mask: isize = unsafe { msg_send![event, buttonMask] };
    mask.cast_unsigned()
}

/// Returns an identifier stable for the lifetime of this UIKit touch.
pub fn touch_id(touch: *mut AnyObject) -> TouchId {
    TouchId(touch as usize as u64)
}

/// Returns normalized pressure when UIKit reports a meaningful force range.
pub fn touch_force(touch: *mut AnyObject) -> Option<f32> {
    unsafe {
        let maximum_force: f64 = msg_send![touch, maximumPossibleForce];
        if maximum_force <= 0.0 {
            return None;
        }

        let force: f64 = msg_send![touch, force];
        Some((force / maximum_force).clamp(0.0, 1.0) as f32)
    }
}

/// Where UIKit predicts `touch` will be about a frame from now, in window coordinates. UIKit
/// only offers predictions while a touch moves; `None` otherwise.
pub fn predicted_touch_location(
    touch: *mut AnyObject,
    event: *mut AnyObject,
    view: *mut AnyObject,
) -> Option<Point<Pixels>> {
    if event.is_null() {
        return None;
    }
    unsafe {
        let predicted: *mut AnyObject = msg_send![event, predictedTouchesForTouch: touch];
        if predicted.is_null() {
            return None;
        }
        let last: *mut AnyObject = msg_send![predicted, lastObject];
        if last.is_null() {
            return None;
        }
        let location: ObjcCGPoint = msg_send![last, locationInView: view];
        Some(Point::new(px(location.x as f32), px(location.y as f32)))
    }
}
