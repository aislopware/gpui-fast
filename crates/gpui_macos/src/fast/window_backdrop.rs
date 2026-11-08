//! The view behind a transparent window's content: `NSVisualEffectView`'s behind-window blur for
//! [`WindowBackgroundAppearance::Blurred`], and Liquid Glass (`NSGlassEffectView`, macOS 26 and
//! later) for [`WindowBackgroundAppearance::Glass`], falling back to the blur where the class is
//! missing. Either fills the content view, below the Metal layer, resizing with it.
//!
//! The glass is the regular style, the frosted variant Apple gives sidebars and other surfaces
//! that carry text, and it follows the person's Reduce Transparency and Increase Contrast settings
//! on its own. Its corner radius is the window's, so its rim follows the window's corners.
use std::ptr;

use gpui::WindowBackgroundAppearance;
use objc2::runtime::{AnyClass, AnyObject, Bool};
use objc2::{msg_send, sel};
use objc2_app_kit::{NSAutoresizingMaskOptions, NSWindowOrderingMode};
use objc2_foundation::{NSInteger, NSRect};

/// `NSGlassEffectViewStyleRegular`.
const GLASS_STYLE_REGULAR: NSInteger = 0;

/// Puts the view `appearance` asks for behind `window`'s content, replacing the one in `view`
/// (with whether it is glass) when it differs. `blur_class` makes the blur view.
///
/// # Safety
///
/// `window` is a live `NSWindow`, on the main thread; `blur_class` is a subclass of
/// `NSVisualEffectView`.
pub(crate) unsafe fn update(
    view: &mut Option<(*mut AnyObject, bool)>,
    window: *mut AnyObject,
    appearance: WindowBackgroundAppearance,
    blur_class: *const AnyClass,
) {
    let glass_class = AnyClass::get(c"NSGlassEffectView");
    let wanted = match appearance {
        WindowBackgroundAppearance::Blurred => Some(false),
        WindowBackgroundAppearance::Glass => Some(glass_class.is_some()),
        WindowBackgroundAppearance::Opaque | WindowBackgroundAppearance::Transparent => None,
    };
    if view.map(|(_, glass)| glass) == wanted {
        return;
    }
    // SAFETY: as the caller promises, on the main thread with a live window; every message is
    // one AppKit documents for the receiver's class.
    unsafe {
        if let Some((old, _)) = view.take() {
            let _: () = msg_send![old, removeFromSuperview];
        }
        let Some(glass) = wanted else {
            return;
        };
        let content: *mut AnyObject = msg_send![window, contentView];
        let frame: NSRect = msg_send![content, bounds];
        let made: *mut AnyObject = match glass_class.filter(|_| glass) {
            Some(class) => {
                let made: *mut AnyObject = msg_send![class, alloc];
                let made: *mut AnyObject = msg_send![made, initWithFrame: frame];
                let _: () = msg_send![made, setStyle: GLASS_STYLE_REGULAR];
                let _: () = msg_send![made, setCornerRadius: corner_radius(window)];
                made
            }
            None => {
                let made: *mut AnyObject = msg_send![blur_class, alloc];
                msg_send![made, initWithFrame: frame]
            }
        };
        let _: () = msg_send![made, setAutoresizingMask: NSAutoresizingMaskOptions::ViewWidthSizable
            | NSAutoresizingMaskOptions::ViewHeightSizable];
        let _: () = msg_send![
            content,
            addSubview: made,
            positioned: NSWindowOrderingMode::Below,
            relativeTo: ptr::null_mut::<AnyObject>()
        ];
        *view = Some((msg_send![made, autorelease], glass));
    }
}

/// `window`'s corner radius in points: AppKit's own, which macOS 26 varies with the window's
/// toolbar style and no public API reports; 0 where the window does not answer.
unsafe fn corner_radius(window: *mut AnyObject) -> f64 {
    // SAFETY: `respondsToSelector:` is NSObject's; `_cornerRadius` is sent only when answered.
    unsafe {
        let answers: Bool = msg_send![window, respondsToSelector: sel!(_cornerRadius)];
        if answers.as_bool() {
            msg_send![window, _cornerRadius]
        } else {
            0.
        }
    }
}
