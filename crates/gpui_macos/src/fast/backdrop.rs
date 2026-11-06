//! The radius of the window's behind-window blur.
//!
//! Taken from zui (zeronsh/zui, Apache-2.0, `crates/gpui_macos/src/window.rs`
//! `remove_layer_background`): `NSVisualEffectView`'s default gaussian radius
//! is tuned for thin translucency, and a window shell tinted at 0.80 over it
//! reads calmer with a wider blur.

use objc2::{msg_send, rc::Retained, runtime::AnyObject};
use objc2_foundation::{NSArray, NSNumber, NSString};

/// The blur radius, in points, that zui sets on the material's blur filter.
const RADIUS: f64 = 60.0;

/// Set the blur filter among `filters`, the effect view's `layer` filters, to
/// [`RADIUS`]. Filters are copied into the render tree, so the array is set
/// on `layer` again for the change to show.
pub(crate) unsafe fn widen_blur(layer: *mut AnyObject, filters: *mut AnyObject) {
    // SAFETY: the caller passes the effect view's live layer and its non-null
    // `filters` array, whose elements are `CAFilter`s answering
    // `description` and key-value coding.
    unsafe {
        let filters = &*filters.cast::<NSArray<AnyObject>>();
        let blur = NSString::from_str("Blur");
        for filter in filters.iter() {
            let description: Retained<NSString> = msg_send![&*filter, description];
            if description.containsString(&blur) {
                let radius = NSNumber::new_f64(RADIUS);
                let key = NSString::from_str("inputRadius");
                let _: () = msg_send![&*filter, setValue: &*radius, forKey: &*key];
                let _: () = msg_send![layer, setFilters: filters];
                break;
            }
        }
    }
}
