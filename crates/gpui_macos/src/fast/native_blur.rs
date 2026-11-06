//! Natives blurred under a backdrop blur, on macOS.
//!
//! GPUI's own blur (`gpui_apple::fast::backdrop`) reads what GPUI drew into its drawable,
//! and where a native shows that is a hole: transparent. A frosted menu over a web page
//! would blur the GPUI content around the page and show the page sharp. So for each blur
//! the presented frame lays over a native ([`NativeFrameBlur`]), a `GPUINativeBlurView`
//! is shown in the window's content view, among the natives' containers under the GPUI
//! view. Its layer is a `CABackdropLayer` with a gaussian `CAFilter`: the window server
//! blurs what is composited behind it in the window, the containers below it and the
//! window's background, live, so a page that scrolls or plays under a still menu stays
//! blurred with no frame of GPUI's.
//!
//! **Stacking.** Containers are stacked by `zPosition` ([`native_z`]): under GPUI's layer
//! at 0, rising with their rank, in (−½, 0) whatever their number. A blur drawn over
//! `natives_below` natives sits between the container of rank `natives_below − 1` and
//! the next ([`blur_z`]), so a native drawn over the menu is not blurred by it. Blurs in
//! one gap keep their draw order, so a blur over another blurs it again, as GPUI's do.
//! GPUI's layer, with its own blur of its content, goes over all of it: where its
//! blurred content is opaque it hides the native blur; where it is transparent (inside
//! the hole) the blurred native shows; at the hole's edge both are soft, and the result,
//! `blur(G) + (1 − blur(α_G)) × blur(N)`, differs from blurring the composite,
//! `blur(G + (1 − α_G) × N)`, only where the native changes within a blur radius of the
//! hole's edge, and there by a soft amount, never a sharp seam.
//!
//! **Geometry.** The view takes the part of the blur's region that shows, its corners
//! (Core Animation rounds every corner it masks by one radius, so the largest is used
//! and the corners its content mask cut off are left square through `maskedCorners`), its
//! radius as the filter's `inputRadius` in points, and its element opacity. An edge fade
//! or rounded clip around the blur is not applied to the native blur. The frame's
//! `CATransaction` carries every change with the containers' and GPUI's drawable, so a
//! blur moves with its hole and its menu on the same refresh; a frame whose blurs did
//! not change touches no view, and a view whose blur did not change is not touched
//! either. Views are kept for reuse, hidden when not needed, and removed with the window.
//!
//! **Private API.** `CABackdropLayer` and `CAFilter` are private Core Animation classes
//! (they are what `NSVisualEffectView` is built from). The public within-window
//! `NSVisualEffectView` is not used: it may blur a cached image of the view hierarchy that
//! omits layer-hosted content such as a web view's, and it lays a material tint the
//! application cannot take off, where the app draws its own translucent fill. Where the
//! classes are missing the view class is not registered and blurs over natives blur
//! GPUI's content alone, as before. `GPUI_NATIVE_BLUR=0` turns native blurs off, to see
//! what they do.
//!
//! Ported from zui (<https://github.com/zeronsh/zui>: `b8f3465`, and its PR #5, `1750e5e`,
//! `b1ebf10`, `a8c7eb0`, `7fe5cc7`, `e116f2e`, `19f06dc`; Apache-2.0, by Wing and the zui
//! contributors; zui is a fork of Zed's GPUI). Changed from zui: zui puts its views under
//! a second GPUI surface laid over the natives and mirrors every overlay blur; here the
//! natives are under GPUI's one layer, so a view is placed only for a blur over a
//! native's hole, stacked among the natives by the frame's ranks, from geometry the frame
//! computes in `gpui::fast::composition`, and only when that geometry changed. Square
//! corners where the content mask cuts the region, and the backdrop's sampling scale
//! following the radius, are this fork's.

use std::{ptr, sync::Once};

use gpui::{Bounds, Pixels, composition::NativeFrameBlur};
use objc2::{
    class, msg_send,
    rc::autoreleasepool,
    runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel},
    sel,
};
use objc2_foundation::{NSNumber, NSPoint, NSRect, NSSize, NSString};

/// Objective-C's `nil`.
const NIL: *mut AnyObject = ptr::null_mut();

/// `GPUINativeBlurView`, or null where Core Animation lacks `CABackdropLayer` or
/// `CAFilter`.
static mut VIEW_CLASS: *const AnyClass = ptr::null();

/// `CACornerMask` bits, in the layer's own coordinates: a view that is not flipped has
/// its layer's origin at the bottom left, so GPUI's top is the layer's maximum y.
const CORNER_BOTTOM_LEFT: usize = 1 << 0;
const CORNER_BOTTOM_RIGHT: usize = 1 << 1;
const CORNER_TOP_LEFT: usize = 1 << 2;
const CORNER_TOP_RIGHT: usize = 1 << 3;

/// Whether blurs over natives are drawn (`GPUI_NATIVE_BLUR=0` turns them off).
fn enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("GPUI_NATIVE_BLUR").map_or(true, |value| value != "0"))
}

/// The view class blurs are shown in, registered on first use; `None` where Core
/// Animation has no backdrop layer.
fn view_class() -> Option<&'static AnyClass> {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| {
        if AnyClass::get(c"CABackdropLayer").is_none() || AnyClass::get(c"CAFilter").is_none() {
            log::warn!("CABackdropLayer is missing: backdrop blurs will not blur natives");
            return;
        }
        let mut decl = ClassBuilder::new(c"GPUINativeBlurView", class!(NSView))
            .expect("GPUINativeBlurView is declared once");
        // SAFETY: the methods' signatures match the selectors' Objective-C types.
        unsafe {
            decl.add_method(
                sel!(makeBackingLayer),
                make_backing_layer as extern "C" fn(_, Sel) -> *mut AnyObject,
            );
            decl.add_method(
                sel!(hitTest:),
                hit_nothing as extern "C" fn(_, Sel, NSPoint) -> *mut AnyObject,
            );
            VIEW_CLASS = decl.register();
        }
    });
    // SAFETY: written once above, before any read.
    let class = unsafe { VIEW_CLASS };
    // SAFETY: null, or the class registered above, which lives for the process.
    unsafe { class.as_ref() }
}

/// `-[NSView makeBackingLayer]`: a backdrop layer, which the window server fills with
/// what is composited behind it, through the filters it is given.
extern "C" fn make_backing_layer(_: &AnyObject, _: Sel) -> *mut AnyObject {
    // SAFETY: the class is registered only where `CABackdropLayer` exists. `+layer`
    // returns an autoreleased layer, which AppKit retains as the view's. Its private
    // setters are sent only where it answers them. Main thread.
    unsafe {
        let layer: *mut AnyObject = msg_send![class!(CABackdropLayer), layer];
        if layer.is_null() {
            return NIL;
        }
        let responds = |selector: Sel| -> bool {
            let responds: Bool = msg_send![layer, respondsToSelector: selector];
            responds.as_bool()
        };
        if responds(sel!(setAllowsGroupBlending:)) {
            let () = msg_send![layer, setAllowsGroupBlending: Bool::YES];
        }
        if responds(sel!(setIgnoresOffscreenGroups:)) {
            let () = msg_send![layer, setIgnoresOffscreenGroups: Bool::YES];
        }
        if responds(sel!(setAllowsInPlaceFiltering:)) {
            let () = msg_send![layer, setAllowsInPlaceFiltering: Bool::NO];
        }
        let () = msg_send![layer, setAllowsGroupOpacity: Bool::YES];
        let () = msg_send![layer, setMasksToBounds: Bool::YES];
        layer
    }
}

/// A blur is only seen: the pointer goes to what is under it.
extern "C" fn hit_nothing(_: &AnyObject, _: Sel, _: NSPoint) -> *mut AnyObject {
    NIL
}

/// The `zPosition` of the container of the native of rank `rank` (its place from the
/// bottom among the frame's natives): under GPUI's layer at 0, rising with the rank, in
/// (−½, 0) whatever the number of natives. A container whose rank did not change keeps
/// its place when a native is placed above it, and the place of a blur between two ranks
/// is known without reading the containers.
pub(crate) fn native_z(rank: usize) -> f64 {
    -1. / (rank as f64 + 2.)
}

/// The `zPosition` of a blur drawn over `natives_below` natives: between the container of
/// rank `natives_below − 1` (or −1, under every container) and that of rank
/// `natives_below`, the `index`th of the `count` blurs in that gap, in draw order.
pub(crate) fn blur_z(natives_below: usize, index: usize, count: usize) -> f64 {
    let low = natives_below.checked_sub(1).map_or(-1., native_z);
    let high = native_z(natives_below);
    low + (high - low) * (index + 1) as f64 / (count + 1) as f64
}

/// The fraction of the layer's resolution the backdrop is sampled at, for a blur of
/// `radius` points: a wide blur hides a coarse sampling, a narrow one does not.
fn sample_scale(radius: f64) -> f64 {
    if radius >= 8. {
        0.25
    } else if radius >= 3. {
        0.5
    } else {
        1.
    }
}

/// One backdrop view and what it shows.
struct BlurView {
    /// Owned: one reference, taken at creation. The content view holds another.
    view: *mut AnyObject,
    /// The blur and `zPosition` it was last given, or `None` while it is hidden.
    shown: Option<(NativeFrameBlur, f64)>,
    /// The gaussian radius its filter has, in points; `None` before the first.
    filter_radius: Option<f64>,
}

/// A window's backdrop views, one per blur over a native in the last frame that changed
/// them, and those kept hidden for reuse.
#[derive(Default)]
pub(crate) struct NativeBlurs {
    views: Vec<BlurView>,
}

impl NativeBlurs {
    /// The backdrop views, shown or not, for tests.
    pub(crate) fn views(&self) -> Vec<*mut AnyObject> {
        self.views.iter().map(|view| view.view).collect()
    }

    /// Shows `blurs`, a frame's blurs over natives in draw order, each in a backdrop view
    /// under `gpui_view` in its superview, and hides the views left over. `frame_of`
    /// converts a rectangle in GPUI's coordinates to the content view's. Called inside the
    /// frame's `CATransaction`, on the main thread.
    pub(crate) fn apply(
        &mut self,
        blurs: &[NativeFrameBlur],
        gpui_view: *mut AnyObject,
        frame_of: impl Fn(Bounds<Pixels>) -> NSRect,
    ) {
        let Some(class) = view_class() else {
            return;
        };
        let blurs = if enabled() { blurs } else { &[] };
        autoreleasepool(|_| {
            for (index, blur) in blurs.iter().enumerate() {
                if index == self.views.len() {
                    let Some(view) = create_view(class, gpui_view) else {
                        return;
                    };
                    self.views.push(BlurView {
                        view,
                        shown: None,
                        filter_radius: None,
                    });
                }
                // Blurs are in draw order, so those over as many natives are adjacent.
                let gap = blurs[..index]
                    .iter()
                    .rposition(|other| other.natives_below != blur.natives_below)
                    .map_or(0, |before| before + 1);
                let count = blurs[gap..]
                    .iter()
                    .take_while(|other| other.natives_below == blur.natives_below)
                    .count();
                let z = blur_z(blur.natives_below, index - gap, count);
                let frame = frame_of(blur.bounds);
                self.views[index].show(blur, z, frame);
            }
            for view in &mut self.views[blurs.len()..] {
                view.hide();
            }
        });
    }
}

/// A backdrop view, hidden, in `gpui_view`'s superview just under `gpui_view`.
fn create_view(class: &AnyClass, gpui_view: *mut AnyObject) -> Option<*mut AnyObject> {
    // SAFETY: AppKit view creation and insertion on the main thread. `alloc`/`init` give
    // the one reference `BlurView` owns; the content view retains its own. Setting
    // `wantsLayer` makes AppKit ask `makeBackingLayer` for the backdrop layer.
    unsafe {
        let view: *mut AnyObject = msg_send![class, alloc];
        let zero = NSRect::new(NSPoint::new(0., 0.), NSSize::new(0., 0.));
        let view: *mut AnyObject = msg_send![view, initWithFrame: zero];
        if view.is_null() {
            return None;
        }
        let () = msg_send![view, setWantsLayer: Bool::YES];
        // The backdrop is rounded only while its layer masks to its bounds; the view clips
        // too, as the containers do, so AppKit's view clipping agrees with the layer.
        let () = msg_send![view, setClipsToBounds: Bool::YES];
        let () = msg_send![view, setHidden: Bool::YES];
        let content_view: *mut AnyObject = msg_send![gpui_view, superview];
        let () =
            msg_send![content_view, addSubview: view, positioned: -1isize, relativeTo: gpui_view];
        Some(view)
    }
}

impl BlurView {
    /// Shows `blur` at `frame` in the content view and at `z`, changing only what differs
    /// from what the view shows.
    fn show(&mut self, blur: &NativeFrameBlur, z: f64, frame: NSRect) {
        if self
            .shown
            .as_ref()
            .is_some_and(|(shown, shown_z)| shown == blur && *shown_z == z)
        {
            return;
        }
        let radii = &blur.corner_radii;
        let corners = [
            (radii.top_left, CORNER_TOP_LEFT),
            (radii.top_right, CORNER_TOP_RIGHT),
            (radii.bottom_right, CORNER_BOTTOM_RIGHT),
            (radii.bottom_left, CORNER_BOTTOM_LEFT),
        ];
        let corner_radius = corners
            .iter()
            .map(|(radius, _)| f64::from(*radius))
            .fold(0., f64::max);
        let masked_corners = corners
            .iter()
            .filter(|(radius, _)| f64::from(*radius) > 0.)
            .fold(0usize, |mask, (_, corner)| mask | corner);
        let radius = f64::from(blur.blur_radius);
        // SAFETY: the view is a live backdrop view of this window, whose layer is a
        // `CABackdropLayer` (`setScale:` is sent only where it answers it); `CAFilter`
        // exists where the view class does. The filter and the array are autoreleased
        // into the caller's pool, and the layer copies its filters. Main thread, inside
        // the frame's transaction.
        unsafe {
            let layer: *mut AnyObject = msg_send![self.view, layer];
            if layer.is_null() {
                return;
            }
            let () = msg_send![self.view, setFrame: frame];
            let () = msg_send![layer, setZPosition: z];
            let () = msg_send![layer, setCornerRadius: corner_radius];
            let () = msg_send![layer, setMaskedCorners: masked_corners];
            let () = msg_send![self.view, setAlphaValue: f64::from(blur.opacity)];
            if self.filter_radius != Some(radius) {
                // A layer copies its filters into the render tree when they are set, so
                // a new filter is set rather than the old one's radius changed.
                let filter: *mut AnyObject = msg_send![
                    class!(CAFilter),
                    filterWithType: &*NSString::from_str("gaussianBlur")
                ];
                if !filter.is_null() {
                    let () = msg_send![
                        filter,
                        setValue: &*NSNumber::new_f64(radius),
                        forKey: &*NSString::from_str("inputRadius")
                    ];
                    let () = msg_send![
                        filter,
                        setValue: &*NSNumber::new_bool(true),
                        forKey: &*NSString::from_str("inputNormalizeEdges")
                    ];
                    let filters: *mut AnyObject =
                        msg_send![class!(NSArray), arrayWithObject: filter];
                    let () = msg_send![layer, setFilters: filters];
                    let responds: Bool = msg_send![layer, respondsToSelector: sel!(setScale:)];
                    if responds.as_bool() {
                        let () = msg_send![layer, setScale: sample_scale(radius)];
                    }
                    self.filter_radius = Some(radius);
                }
            }
            if self.shown.is_none() {
                let () = msg_send![self.view, setHidden: Bool::NO];
            }
        }
        self.shown = Some((blur.clone(), z));
    }

    fn hide(&mut self) {
        if self.shown.take().is_some() {
            // SAFETY: the view is live; main thread, inside the frame's transaction.
            unsafe {
                let () = msg_send![self.view, setHidden: Bool::YES];
            }
        }
    }
}

impl Drop for BlurView {
    fn drop(&mut self) {
        // SAFETY: the view holds the one reference taken at creation; main thread, where
        // the window's composition is dropped.
        unsafe {
            let () = msg_send![self.view, removeFromSuperview];
            let () = msg_send![self.view, release];
        }
    }
}
