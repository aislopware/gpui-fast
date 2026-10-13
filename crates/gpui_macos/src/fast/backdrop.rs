//! The radius of the window's behind-window blur.
//!
//! Taken from zui (zeronsh/zui, Apache-2.0, `crates/gpui_macos/src/window.rs`
//! `remove_layer_background`): `NSVisualEffectView`'s default gaussian radius
//! is tuned for thin translucency, and a window shell tinted at 0.80 over it
//! reads calmer with a wider blur.

use cocoa::{
    base::{BOOL, YES, id},
    foundation::NSArray,
};
use objc::{class, msg_send, sel, sel_impl};

use crate::ns_string;

/// The blur radius, in points, that zui sets on the material's blur filter.
const RADIUS: f64 = 60.0;

/// Set the blur filter among `filters`, the effect view's `layer` filters, to
/// [`RADIUS`]. Filters are copied into the render tree, so the array is set
/// on `layer` again for the change to show.
pub(crate) unsafe fn widen_blur(layer: id, filters: id) {
    // SAFETY: the caller passes the effect view's live layer and its non-null
    // `filters` array, whose elements are `CAFilter`s answering
    // `description` and key-value coding.
    unsafe {
        let blur = ns_string("Blur");
        let count = NSArray::count(filters);
        for i in 0..count {
            let filter = filters.objectAtIndex(i);
            let description: id = msg_send![filter, description];
            let hit: BOOL = msg_send![description, containsString: blur];
            if hit == YES {
                let radius: id = msg_send![class!(NSNumber), numberWithDouble: RADIUS];
                let _: () = msg_send![filter, setValue: radius forKey: ns_string("inputRadius")];
                let _: () = msg_send![layer, setFilters: filters];
                break;
            }
        }
    }
}
