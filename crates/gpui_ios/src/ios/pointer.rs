//! The iPad's pointer over the window's view: a `UIPointerInteraction` whose delegate (the
//! metal view) answers with the [`PointerLook`] the platform's `set_cursor_style` last set,
//! built as a `UIPointerStyle` (see `crate::pointer` for the looks and why).

use std::cell::Cell;
use std::ptr;

use objc2::runtime::{AnyObject, Sel};
use objc2::{class, msg_send};

use super::cg_types::ObjcCGPoint;
use crate::pointer::{BEAM_LENGTH, PointerLook, outline};

/// `UIAxisHorizontal`.
const AXIS_HORIZONTAL: usize = 1 << 0;
/// `UIAxisVertical`.
const AXIS_VERTICAL: usize = 1 << 1;
/// `UIAxisNeither`: the pointer moves freely in the shape.
const AXIS_NEITHER: usize = 0;

thread_local! {
    /// The look the pointer takes over every window's view. Main thread only, as UIKit is.
    static LOOK: Cell<PointerLook> = const { Cell::new(PointerLook::System) };
}

/// The pointer takes `look` from now on: each window's interaction asks its style again when
/// the look is a new one.
pub(crate) fn set_look(look: PointerLook) {
    if LOOK.replace(look) == look {
        return;
    }
    super::ffi::for_each_window(|window| window.invalidate_pointer());
}

/// The look in force.
#[cfg(feature = "test-support")]
pub(crate) fn look() -> PointerLook {
    LOOK.get()
}

/// A pointer interaction on `view`, its delegate the view itself, added to it. Retained.
///
/// # Safety
///
/// `view` must be a live `GPUIMetalView`, which answers `pointerInteraction:styleForRegion:`,
/// on the main thread.
pub(crate) unsafe fn add_interaction(view: *mut AnyObject) -> *mut AnyObject {
    // SAFETY: UIKit's `UIPointerInteraction` rule: `initWithDelegate:` keeps the delegate
    // weakly, and the view outlives its interaction, which it holds; `addInteraction:` is
    // UIView's, on the main thread.
    unsafe {
        let interaction: *mut AnyObject = msg_send![class!(UIPointerInteraction), alloc];
        let interaction: *mut AnyObject = msg_send![interaction, initWithDelegate: view];
        let _: () = msg_send![view, addInteraction: interaction];
        interaction
    }
}

/// `pointerInteraction:styleForRegion:` on the metal view: the style of the look in force,
/// nil for iPadOS's own pointer.
pub(crate) extern "C" fn style_for_region(
    _this: *mut AnyObject,
    _sel: Sel,
    _interaction: *mut AnyObject,
    _region: *mut AnyObject,
) -> *mut AnyObject {
    // SAFETY: UIKit calls the delegate on the main thread, and every class and selector below
    // is `UIPointerStyle`'s, `UIPointerShape`'s or `UIBezierPath`'s public API (iOS 13.4+);
    // the objects come autoreleased from their class constructors, which is what the
    // delegate method returns.
    unsafe { style_of(LOOK.get()) }
}

/// The `UIPointerStyle` of `look`, autoreleased; nil for iPadOS's own pointer.
///
/// # Safety
///
/// Main thread only.
unsafe fn style_of(look: PointerLook) -> *mut AnyObject {
    // SAFETY: as `style_for_region` says.
    unsafe {
        let shape: *mut AnyObject = match look {
            PointerLook::System => return ptr::null_mut(),
            PointerLook::Hidden => return msg_send![class!(UIPointerStyle), hiddenPointerStyle],
            PointerLook::Beam { upright } => {
                let axis = if upright {
                    AXIS_VERTICAL
                } else {
                    AXIS_HORIZONTAL
                };
                msg_send![class!(UIPointerShape), beamWithPreferredLength: BEAM_LENGTH, axis: axis]
            }
            PointerLook::Outline(shape) => {
                let path: *mut AnyObject = msg_send![class!(UIBezierPath), bezierPath];
                for (i, (x, y)) in outline(shape).into_iter().enumerate() {
                    let point = ObjcCGPoint { x, y };
                    if i == 0 {
                        let _: () = msg_send![path, moveToPoint: point];
                    } else {
                        let _: () = msg_send![path, addLineToPoint: point];
                    }
                }
                let _: () = msg_send![path, closePath];
                msg_send![class!(UIPointerShape), shapeWithPath: path]
            }
        };
        msg_send![class!(UIPointerStyle), styleWithShape: shape, constrainedAxes: AXIS_NEITHER]
    }
}
